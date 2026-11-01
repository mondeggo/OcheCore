"use strict";

/* Debug tools: drive the board by hand, throw demo darts, read the raw event
   log and the daemon's saved configuration. Loaded after assets/app.js,
   whose helpers it uses. Each [data-command] button sends that command. */

/* The configuration as the keys `PUT /api/v1/config` accepts. */
function configEntries(config) {
  const entries = [
    ["board.id", config.board.id],
    ["board.name", config.board.name],
    ["server.bind", config.server.bind],
    ["server.port", config.server.port],
    ["detection.speed", config.detection.speed],
    ["detection.standby_seconds", config.detection.standby_seconds],
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
