"use strict";

/* OcheCore's browser UI, served by the daemon on its own port. The daemon is
   the only source of truth: pages render the snapshots it streams on
   /api/v1/events and change it only through /api/v1 commands and config
   changes (docs/native-api.md). localStorage keeps per-browser conveniences. */

const SEGMENTS = [20, 1, 18, 4, 13, 6, 10, 15, 2, 17, 3, 19, 7, 16, 8, 11, 14, 9, 12, 5];
/* Nominal WDF radii in millimetres, like BoardDimensions::wdf in oche-domain. */
const R = {
  bull: 6.35,
  outerBull: 15.9,
  tripleIn: 99.4,
  tripleOut: 107.4,
  doubleIn: 162,
  doubleOut: 170
};
const COLORS = {
  dark: "#20241f",
  light: "#efe2bd",
  red: "#d84c40",
  green: "#267757",
  surround: "#101c17",
  wire: "#b9c3be",
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
  left: '<path d="M15 5l-7 7 7 7"/>',
  right: '<path d="M9 5l7 7-7 7"/>'
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

  if (surround) polygon(circlePoints(228), COLORS.surround);

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

const NOMINAL_RADII = [R.bull, R.outerBull, R.tripleIn, R.tripleOut, R.doubleIn, R.doubleOut];

function drawWires(parent, map = identity, attrs = {}, radii = NOMINAL_RADII) {
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

function svgPoint(svg, event, target = svg) {
  const point = svg.createSVGPoint();
  point.x = event.clientX;
  point.y = event.clientY;
  return point.matrixTransform(target.getScreenCTM().inverse());
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
      return snapshot.throws.length >= 3
        ? make("attention", "Take out darts", "the visit is complete; remove the darts")
        : make("ready", "Ready", snapshot.physical_darts == null ? "" : `${snapshot.physical_darts} on the board`);
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
  const camerasDone = cameras.every(camera => camera.device);
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

function renderStatusBar({ snapshot, connected }) {
  const state = document.getElementById("status-state");
  const cameraText = document.getElementById("status-cameras");
  const address = document.getElementById("status-address");
  const connection = document.getElementById("status-connection");
  if (!state) return;
  if (!snapshot) {
    state.textContent = "Connecting";
    state.dataset.tone = "busy";
    if (connection) connection.textContent = "Waiting for OcheCore…";
    return;
  }
  const activity = boardActivity(snapshot);
  state.textContent = connected ? activity.label : "Offline";
  state.dataset.tone = connected ? LEVEL_TONES[activity.level] : "problem";
  state.title = connected ? sentence(activity.detail) : "The daemon is not reachable; retrying";
  if (cameraText) {
    const count = snapshot.config.cameras.length;
    const streaming = snapshot.cameras.filter(status => status.state === "streaming");
    const fps = streaming.length
      ? ` · ${(streaming.reduce((sum, status) => sum + status.fps, 0) / streaming.length).toFixed(0)} fps`
      : "";
    cameraText.textContent = `${count} ${count === 1 ? "camera" : "cameras"}${fps}`;
  }
  if (address) {
    const { bind, port } = snapshot.config.server;
    const saved = bind.includes(":") ? `[${bind}]:${port}` : `${bind}:${port}`;
    address.textContent = snapshot.restart_required
      ? `${snapshot.listening_on} · ${saved} after restart`
      : snapshot.listening_on;
  }
  if (connection) {
    connection.textContent = connected ? MODE_NAMES[snapshot.mode] || snapshot.mode : "Disconnected · retrying";
  }
}

/* ---------- Camera previews ---------- */

/* One camera's latest frame, turned upright by its configured rotation.
   Frames arrive unrotated, so drawings and clicks use raw image pixels in
   `frame`, the group that carries the rotation. */
function createCameraPreview(slot) {
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
    element,
    svg,
    frame,
    overlay,
    label,
    layout({ width, height, rotation }) {
      const key = `${width}x${height}@${rotation}`;
      if (key === layoutKey) return;
      layoutKey = key;
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
    },
    /* A message instead of the frame, with an optional Start button. */
    say(text, start = false) {
      const html = text
        ? `<p>${escapeHtml(text)}</p>${start ? '<button class="btn btn-small btn-lime" type="button" data-command="start">Start cameras</button>' : ""}`
        : "";
      if (message.innerHTML !== html) message.innerHTML = html;
      message.hidden = !text;
      svg.style.visibility = text ? "hidden" : "";
    },
    /* Image pixel under a click. */
    pixelAt(event) {
      const point = svgPoint(svg, event, frame);
      return [Math.round(point.x), Math.round(point.y)];
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

/* Fetches each visible camera's latest JPEG, one request per camera at a time. */
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

  tick() {
    if (document.hidden || !daemon.snapshot || !daemon.connected) return;
    this.previews.forEach((_, slot) => this.fetch(slot));
  },

  async fetch(slot, force = false) {
    const preview = this.previews.get(slot);
    if (!preview || this.pending.has(slot) || (!force && !isStreaming(daemon.snapshot, slot))) return;
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

/* Same rules as Config::validate in oche-domain, checked before sending. */
function validateBoard({ name, bind, port }) {
  const errors = {};
  if (!name) errors.name = "Enter a name.";
  else if ([...name].length > 80) errors.name = "Use at most 80 characters.";
  else if (/[\u0000-\u001f\u007f-\u009f]/.test(name)) errors.name = "Use printable characters only.";
  if (!isIpAddress(bind)) errors.bind = "Enter an IP address such as 127.0.0.1 or 0.0.0.0.";
  if (!/^\d{1,5}$/.test(port) || Number(port) < 1 || Number(port) > 65535) {
    errors.port = "Enter a port between 1 and 65535.";
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

function initBoardPage() {
  const form = document.getElementById("board-form");
  const inputs = {
    "board.name": document.getElementById("board-name"),
    "server.bind": document.getElementById("host-ip"),
    "server.port": document.getElementById("host-port")
  };
  const next = document.querySelector("[data-save-next]");
  let saving = false;

  whenReady(snapshot => {
    enterStep(1, snapshot);
    const { board, server } = snapshot.config;
    inputs["board.name"].value = board.name;
    inputs["server.bind"].value = server.bind;
    inputs["server.port"].value = server.port;
  });
  daemon.subscribe(({ snapshot }) => setStepStates(1, snapshot));

  async function save() {
    const { config } = daemon.snapshot;
    const data = {
      name: inputs["board.name"].value.trim(),
      bind: inputs["server.bind"].value.trim(),
      port: inputs["server.port"].value.trim()
    };
    const errors = validateBoard(data);
    setFieldError(inputs["board.name"], errors.name);
    setFieldError(inputs["server.bind"], errors.bind);
    setFieldError(inputs["server.port"], errors.port);
    const invalid = Object.values(inputs).find(input => input.hasAttribute("aria-invalid"));
    if (invalid) {
      invalid.focus();
      return false;
    }
    const changes = [
      ["board.name", data.name, config.board.name],
      ["server.bind", data.bind, config.server.bind],
      ["server.port", Number(data.port), config.server.port]
    ].filter(([, value, current]) => String(value) !== String(current));
    try {
      const snapshot = await api.configure(changes);
      if (snapshot?.restart_required) toast("Saved. The new address applies after OcheCore restarts.");
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

  async function saveAndContinue() {
    if (saving || !daemon.snapshot) return;
    saving = true;
    if (await save()) window.location.href = next.href;
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

const deviceMatches = (device, path) => device.path === path || (device.aliases || []).includes(path);

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

  /* Sends changes, then re-renders from the daemon's reply. */
  async function apply(changes, success) {
    if (busy || !changes.length) return;
    busy = true;
    grid.classList.add("is-busy");
    await attempt(api.configure(changes), success);
    busy = false;
    grid.classList.remove("is-busy");
    renderedKey = "";
    render(daemon);
  }

  function deviceOptions(camera) {
    const known = inventory.devices.find(device => deviceMatches(device, camera.device));
    const list = [["", "Choose a device"]];
    inventory.devices.forEach(device => {
      const value = known === device ? camera.device : device.path;
      list.push([value, `${device.name || "Camera"} · ${device.path}${device.error ? " (error)" : ""}`]);
    });
    if (camera.device && !known) list.push([camera.device, `${camera.device} (not found)`]);
    return options(list, camera.device);
  }

  function cardMarkup(snapshot, camera) {
    const { slot } = camera;
    const count = snapshot.config.cameras.length;
    const tools = `
      <div class="camera-tool-buttons">
        <button class="icon-btn" type="button" data-rotate="${slot}" aria-label="Rotate camera ${slot}" title="Rotate 90°">${icon("rotate")}</button>
        <button class="icon-btn" type="button" data-refresh="${slot}" aria-label="Refresh camera ${slot}" title="Refresh preview and device list">${icon("refresh")}</button>
        ${count > 1 ? `<button class="icon-btn" type="button" data-remove="${slot}" aria-label="Remove camera ${slot}" title="Remove camera">${icon("remove")}</button>` : ""}
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

      const choices = cameraChoices(inventory, cameras);
      document.getElementById("camera-resolution").innerHTML =
        options(choices.resolutions.map(size => [size, size]), `${cameras[0].width}x${cameras[0].height}`);
      document.getElementById("camera-fps").innerHTML =
        options(choices.frameRates.map(fps => [fps, `${fps} fps`]), cameras[0].fps);
      document.getElementById("detection-speed").innerHTML = options(SPEEDS, detection.speed);
      document.getElementById("detection-standby").innerHTML = options(STANDBY, detection.standby_seconds);
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

    const missing = needsCameras(snapshot) && cameras.find(camera => !camera.device);
    setNextEnabled(next, !missing, missing ? `Choose a device for camera ${missing.slot}.` : "");
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

  grid.addEventListener("change", event => {
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
    apply([[`cameras.${slot}.device`, path], [`cameras.${slot}.hint`, ""]]);
  });

  settings.addEventListener("change", event => {
    const snapshot = daemon.snapshot;
    if (!snapshot) return;
    const { cameras } = snapshot.config;
    const { id, value } = event.target;
    if (id === "camera-resolution") {
      const [width, height] = value.split("x").map(Number);
      apply(cameras.flatMap(camera => {
        const sameShape = Math.abs(width / height - camera.width / camera.height) < 0.01;
        return [
          [`cameras.${camera.slot}.resolution`, value],
          ...hintChange(snapshot, camera, sameShape ? [width / camera.width, height / camera.height] : null)
        ];
      }));
    }
    if (id === "camera-fps") apply(cameras.map(camera => [`cameras.${camera.slot}.fps`, value]));
    if (id === "detection-speed") apply([["detection.speed", value]]);
    if (id === "detection-standby") apply([["detection.standby_seconds", value]]);
  });

  whenReady(snapshot => {
    if (!enterStep(2, snapshot)) return;
    daemon.subscribe(render);
    discover();
  });
}

/* ---------- Step 3: calibrate ---------- */

/* Two ways to find the 20 on each camera:
   - "pick", on screen (the default): click the 20 on each camera, which sets
     cameras.<slot>.hint, then calibrate. The fitted wires and the 20 in red
     are drawn over each camera; the arrows move a camera's hint to the next
     segment and refit that camera alone.
   - "dart", dart at D20, as in the terminal: orient_begin, one dart at D20,
     orient_finish, then calibrate with the board empty. */
const CALIBRATION_MODES = {
  pick: {
    lede: "Click the 20 on each camera, then OcheCore fits the wires. Check the red segment is the 20 everywhere.",
    blocked: "Calibrate every camera first."
  },
  dart: {
    lede: "Throw one dart at D20, like in the terminal: OcheCore finds the 20 on every camera, then fits the wires.",
    blocked: "Calibrate every camera first."
  }
};

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
    /* The camera being measured, "all", or null. */
    busy: null
  };
  let renderedKey = "";

  const setOrienting = value => {
    ui.orienting = value;
    if (value) session.set("orienting", value);
    else session.remove("orienting");
  };

  /* Where the procedure is, from the daemon's state and the steps run here. */
  function progress(snapshot) {
    const cameras = assignedCameras(snapshot);
    const calibrated = cameras.length > 0 &&
      snapshot.config.cameras.every(camera => calibrationFor(snapshot.calibration, camera));
    const oriented = camerasNeedingOrientation(snapshot).length === 0;
    const orienting = ui.orienting === snapshot.session_id;
    if (ui.mode === "pick") {
      if (calibrated && !ui.redo) return { calibrated, oriented, stage: 2 };
      return { calibrated, oriented, stage: oriented ? 1 : 0 };
    }
    if (calibrated && !ui.redo) return { calibrated, oriented, stage: 3 };
    if (orienting) return { calibrated, oriented, stage: 1 };
    return { calibrated, oriented, stage: (oriented && !ui.redo) || ui.found ? 2 : 0 };
  }

  function chip(snapshot, camera, state) {
    if (!camera.device) return ["No device", "wait"];
    if (ui.busy === "all" || ui.busy === camera.slot) return ["Measuring…", "busy"];
    const calibration = calibrationFor(snapshot.calibration, camera);
    if (calibration && !ui.redo) return [`Wire fit ✓ ${calibration.wire_rms_mm.toFixed(2)} mm`, "done"];
    if (state.stage === 1 && ui.mode === "dart") return ["Waiting for dart", "wait"];
    if (camera.hint || calibration) return ["20 marked", "found"];
    return [ui.mode === "pick" ? "Click the 20" : "Waiting", "wait"];
  }

  function preview(slot) {
    if (!previews.has(slot)) {
      const created = createCameraPreview(slot);
      previews.set(slot, created);
      cameraFeed.watch(created);
    }
    return previews.get(slot);
  }

  /* Wires and the 20 in red where calibrated; the 20 that will be used,
     as a yellow dot, otherwise. */
  function drawOverlay(snapshot, camera, view) {
    view.overlay.replaceChildren();
    const calibration = calibrationFor(snapshot.calibration, camera);
    const map = calibration && !ui.redo && boardToImage(calibration);
    const scale = Math.max(camera.width, camera.height) / 640;
    if (map) {
      const b = snapshot.calibration.board;
      svgEl("polygon", {
        points: pointsAttr(sectorPoints(b.outer_bull, b.double_outer, -99, -81).map(map)),
        fill: COLORS.red,
        opacity: 0.6,
        stroke: COLORS.yellow,
        "stroke-width": 1.5 * scale
      }, view.overlay);
      drawWires(view.overlay, map, { stroke: COLORS.lime, "stroke-width": 1.2 * scale, opacity: 0.9 },
        [b.inner_bull, b.outer_bull, b.treble_inner, b.treble_outer, b.double_inner, b.double_outer]);
      return;
    }
    const hint = camera.hint || (calibration && hintAt(calibration, 0));
    if (hint) {
      svgEl("circle", {
        cx: hint[0], cy: hint[1], r: 9 * scale,
        fill: COLORS.yellow, stroke: COLORS.ink, "stroke-width": 3 * scale
      }, view.overlay);
    }
  }

  function cardMarkup(snapshot, camera, state) {
    const { slot } = camera;
    const count = snapshot.config.cameras.length;
    const calibration = calibrationFor(snapshot.calibration, camera);
    const canTurn = ui.mode === "pick" && state.stage === 2 && calibration;
    return `
      <section class="panel camera-card" aria-labelledby="camera-title-${slot}">
        ${cameraCardHead(slot, count, '<span class="cal-status" data-cal-status></span>')}
        <div data-preview-slot="${slot}"></div>
        ${canTurn ? `
          <div class="orient-controls">
            <span>Turn the red to the 20</span>
            <div class="turn-buttons" role="group" aria-label="Turn the red segment on camera ${slot}">
              <button type="button" data-turn="-1" data-camera="${slot}" aria-label="One segment anticlockwise">${icon("left")}</button>
              <button type="button" data-turn="1" data-camera="${slot}" aria-label="One segment clockwise">${icon("right")}</button>
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
    buttons.fit.disabled = blocked || state.stage !== 1;
    buttons.begin.disabled = blocked || state.stage !== 0;
    buttons.find.disabled = blocked || state.stage !== 1;
    buttons.calibrate.disabled = blocked || state.stage !== 2;
    resetButton.disabled = Boolean(ui.busy) || !live;
    setStepStates(3, snapshot);
    setNextEnabled(next, !live || (state.calibrated && !ui.busy), CALIBRATION_MODES[ui.mode].blocked);
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
        drawOverlay(snapshot, camera, view);
        /* Clicking marks the 20 while the procedure is open in pick mode. */
        view.element.classList.toggle("is-pickable", ui.mode === "pick" && state.stage < 2 && !ui.busy);
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

  async function measure(camera, success) {
    ui.busy = camera ?? "all";
    render(daemon);
    const result = await attempt(api.command("calibrate", camera ? { camera } : {}), success);
    if (result) {
      ui.redo = false;
      ui.found = false;
    }
    ui.busy = null;
    renderedKey = "";
    render(daemon);
    return result;
  }

  modeButtons.forEach(button => {
    button.addEventListener("click", () => {
      if (button.dataset.calibrationMode === ui.mode || ui.busy) return;
      ui.mode = button.dataset.calibrationMode;
      storage.set("calibration-mode", ui.mode);
      setOrienting(null);
      ui.found = false;
      render(daemon);
    });
  });

  buttons.fit.addEventListener("click", () =>
    measure(null, "Calibrated. Check the red segment is the 20 on every camera."));

  buttons.begin.addEventListener("click", async () => {
    ui.busy = "all";
    render(daemon);
    const result = await attempt(api.command("orient_begin"), "Now throw one dart at D20 and leave it in.");
    ui.busy = null;
    if (result) setOrienting(result.session_id);
    render(daemon);
  });

  buttons.find.addEventListener("click", async () => {
    ui.busy = "all";
    render(daemon);
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
    ui.busy = null;
    renderedKey = "";
    render(daemon);
  });

  buttons.calibrate.addEventListener("click", () =>
    measure(null, "Calibrated. Check the red segment is the 20 on every camera."));

  grid.addEventListener("click", async event => {
    const snapshot = daemon.snapshot;
    if (!snapshot || ui.busy) return;
    const turn = event.target.closest("[data-turn]");
    if (turn) {
      /* Move this camera's 20 one segment and refit it against the measured board. */
      const slot = Number(turn.dataset.camera);
      const camera = snapshot.config.cameras[slot - 1];
      const hint = hintAt(calibrationFor(snapshot.calibration, camera), Number(turn.dataset.turn) * 18);
      if (!hint || hint[0] >= camera.width || hint[1] >= camera.height) {
        toast("That segment is outside this camera's view.");
        return;
      }
      ui.busy = slot;
      render(daemon);
      const saved = await attempt(api.setConfig(`cameras.${slot}.hint`, hint.join(",")));
      ui.busy = null;
      if (saved) await measure(slot, `Camera ${slot} refitted.`);
      else render(daemon);
      return;
    }
    const target = event.target.closest(".camera-preview.is-pickable");
    if (!target || event.target.closest("button")) return;
    const slot = Number(target.dataset.cameraPreview);
    const camera = snapshot.config.cameras[slot - 1];
    if (!camera?.device || !isStreaming(snapshot, slot)) return;
    const [x, y] = previews.get(slot).pixelAt(event);
    if (x < 0 || y < 0 || x >= camera.width || y >= camera.height) return;
    const saved = await attempt(api.setConfig(`cameras.${slot}.hint`, `${x},${y}`), `Camera ${slot}: 20 marked.`);
    if (saved) {
      renderedKey = "";
      render(daemon);
    }
  });

  resetButton.addEventListener("click", () => {
    ui.redo = true;
    ui.found = false;
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

/* Shared by the test step, the board view and debug; `interactive` boards
   take demo throws. */
function createPlayView(interactive = false) {
  const board = document.getElementById("throw-board");
  drawBoard(board, identity, { numberSize: 22 });
  const hitLayer = svgEl("g", { "pointer-events": "none" }, board);
  const dartLayer = svgEl("g", { "pointer-events": "none" }, board);
  const slots = document.getElementById("dart-slots");
  const total = document.getElementById("visit-total");
  const boardState = document.getElementById("board-state");

  function render(snapshot) {
    const { throws } = snapshot;
    slots.innerHTML = [0, 1, 2].map(index => {
      const dart = throws[index];
      if (!dart) return '<span class="dart-slot">–</span>';
      const title = dart.uncertain ? ` title="${escapeHtml(dart.uncertainty || "Position uncertain")}"` : "";
      return `<span class="dart-slot is-filled${dart.uncertain ? " is-uncertain" : ""}"${title}>${escapeHtml(dart.label)}</span>`;
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
      const point = labelPoint(dart.label);
      if (!point) return;
      const area = hitArea(...point);
      if (area) svgEl("polygon", { points: pointsAttr(area), fill: COLORS.yellow, opacity: 0.85 }, hitLayer);
      const group = svgEl("g", { transform: `translate(${round1(point[0])} ${round1(point[1])})` }, dartLayer);
      svgEl("circle", { r: 7.4, fill: DART_COLORS[index], stroke: COLORS.ink, "stroke-width": 2 }, group);
      const number = svgEl("text", {
        y: -13,
        "text-anchor": "middle",
        fill: "#fffdf7",
        "font-size": 14,
        "font-weight": 900,
        "font-family": "Arial Rounded MT Bold, Trebuchet MS, sans-serif",
        "paint-order": "stroke",
        stroke: COLORS.ink,
        "stroke-width": 4
      }, group);
      number.textContent = index + 1;
    });
  }

  return { board, render };
}

/* In demo mode, clicking the board or a quick-throw button injects a throw. */
function bindDemoThrows(view, onThrow = () => {}) {
  const send = async label => {
    if (!daemon.snapshot?.capabilities.synthetic_input) return;
    if (await attempt(api.command("demo_throw", { label }))) onThrow();
  };
  view.board.addEventListener("click", event => {
    const point = svgPoint(view.board, event);
    send(scoreAt(point.x, point.y));
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

function initHomePage() {
  const feed = document.getElementById("event-feed");
  const empty = document.getElementById("event-empty");
  whenReady(snapshot => {
    if (!isSetUp(snapshot)) {
      window.location.replace(`setup/${STEP_PAGES[firstOpenStep(snapshot) - 1]}`);
      return;
    }
    document.body.classList.add("is-ready");
    const view = createPlayView();
    daemon.subscribe(({ snapshot: current, events }) => {
      document.getElementById("home-board-name").textContent = current.config.board.name;
      view.render(current);
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
  daemon.start();
});
