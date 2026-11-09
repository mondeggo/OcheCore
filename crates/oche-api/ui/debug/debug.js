"use strict";

/* Debug tools: drive the board by hand, throw demo darts, read the raw event
   log and the daemon's saved configuration. Loaded after assets/app.js,
   whose helpers it uses. Each [data-command] button sends that command. */

/* Events worth a new picture from every camera: a dart, a takeout, the board
   becoming ready, a new calibration. Pictures are not streamed otherwise. */
const PICTURE_EVENTS = new Set([
  "throw_accepted",
  "throw_uncertain",
  "arrival_ignored",
  "partial_takeout",
  "takeout_completed",
  "board_ready",
  "board_not_empty",
  "calibration_updated"
]);

/* Still pictures from each camera, retaken on PICTURE_EVENTS and by hand. */
function initDebugPictures() {
  const board = document.getElementById("visit-board");
  const pictures = document.getElementById("visit-cameras");
  const grid = document.getElementById("debug-camera-grid");
  const caption = document.getElementById("pictures-caption");
  const switches = [...document.querySelectorAll("[data-visit-view]")];
  const previews = new Map();
  let view = storage.get("debug-visit-view", "board");
  let lastEvent = null;
  let layoutKey = "";

  function show(next) {
    view = next;
    storage.set("debug-visit-view", view);
    switches.forEach(button => button.setAttribute("aria-checked", String(button.dataset.visitView === view)));
    board.hidden = view !== "board";
    pictures.hidden = view !== "cameras";
  }

  function retake(why) {
    previews.forEach((_, slot) => cameraFeed.retake(slot));
    caption.textContent = `${why} · ${new Date().toLocaleTimeString()}`;
  }

  function render({ snapshot, events }) {
    const key = JSON.stringify(snapshot.config.cameras);
    if (key !== layoutKey) {
      layoutKey = key;
      grid.replaceChildren();
      [...previews.keys()].forEach(slot => cameraFeed.forget(slot));
      previews.clear();
      snapshot.config.cameras.forEach(camera => {
        const preview = createCameraPreview(camera.slot, true);
        const figure = document.createElement("figure");
        figure.className = "debug-camera";
        const label = document.createElement("figcaption");
        label.textContent = `Camera ${camera.slot}`;
        figure.append(preview.element, label);
        grid.append(figure);
        preview.layout(camera);
        previews.set(camera.slot, preview);
        cameraFeed.watch(preview);
        cameraFeed.fetch(camera.slot);
      });
    }
    snapshot.config.cameras.forEach(camera => {
      const preview = previews.get(camera.slot);
      const [problem, start] = previewProblem(snapshot, camera);
      preview.say(problem, start);
      preview.label.textContent = `${camera.height}p · ${camera.rotation}°`;
    });
    /* New pictures for the newest important event since the last look. */
    const latest = [...events].reverse().find(event => PICTURE_EVENTS.has(event.kind));
    const id = latest ? `${latest.session_id}:${latest.sequence}` : "none";
    /* Events from before this page opened are already in the first pictures. */
    if (lastEvent !== null && id !== lastEvent && latest) {
      retake(`After ${latest.kind.replace(/_/g, " ")} #${latest.sequence}`);
    }
    lastEvent = id;
  }

  switches.forEach(button => button.addEventListener("click", () => show(button.dataset.visitView)));
  document.getElementById("take-pictures").addEventListener("click", () => retake("Taken by hand"));
  show(view);
  daemon.subscribe(render);
  caption.textContent = "Latest pictures. They change after a dart or a takeout, or with Take pictures.";
}

/* A short summary of what an event changed. */
function eventDetail(event) {
  const { snapshot } = event;
  const detail = { state: snapshot.state, scene: snapshot.scene };
  if (snapshot.throws.length) detail.throws = snapshot.throws.map(dart => dart.label);
  if (snapshot.physical_darts != null) detail.on_board = snapshot.physical_darts;
  return detail;
}

function initDebugPage() {
  const log = document.getElementById("event-log");
  const configList = document.getElementById("config-list");
  /* Events up to this one are hidden by Clear, in this browser only. */
  let clearedAt = session.get("debug-cleared", null);
  let configKey = "";

  whenReady(() => {
    initDebugPictures();
    const view = createPlayView(true);
    bindDemoThrows(view);
    daemon.subscribe(({ snapshot, events }) => {
      view.render(snapshot);
      renderControls(snapshot);

      const key = JSON.stringify(snapshot.config);
      if (key !== configKey) {
        configKey = key;
        configList.innerHTML = configEntries(snapshot.config)
          .map(([name, value]) => `<li><code>${escapeHtml(name)}</code><span>${value === "" ? "(none)" : escapeHtml(value)}</span></li>`)
          .join("");
      }

      const visible = events.filter(event =>
        !clearedAt || event.session_id !== clearedAt.session || event.sequence > clearedAt.sequence);
      log.innerHTML = visible.slice().reverse().map(event => {
        const json = escapeHtml(JSON.stringify(eventDetail(event)));
        return `
          <li>
            <span class="log-seq">#${event.sequence}</span>
            <span class="log-kind">${escapeHtml(event.kind)}</span>
            <code title="${json}">${json}</code>
            <time>${timeOf(event)}</time>
          </li>`;
      }).join("") || '<li class="debug-empty">No events yet.</li>';
    });
  });

  document.getElementById("clear-events").addEventListener("click", () => {
    const last = daemon.events[daemon.events.length - 1];
    clearedAt = last ? { session: last.session_id, sequence: last.sequence } : null;
    session.set("debug-cleared", clearedAt);
    daemon.notify();
  });

  document.getElementById("copy-config").addEventListener("click", async () => {
    if (!daemon.snapshot) return;
    try {
      await navigator.clipboard.writeText(JSON.stringify(daemon.snapshot.config, null, 2));
      toast("Configuration copied as JSON.");
    } catch {
      toast("Copying is blocked here. Select the list instead.");
    }
  });
}

document.addEventListener("DOMContentLoaded", () => {
  if (document.body.dataset.page === "debug") initDebugPage();
});
