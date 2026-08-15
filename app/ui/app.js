"use strict";
// PFlowBench frontend. Vanilla JS + vendored uPlot, no build step.
// Talks to the Rust backend via window.__TAURI__ (withGlobalTauri).

const invoke = window.__TAURI__.core.invoke;
const $ = (id) => document.getElementById(id);

// An uncaught exception during init stops this script dead -- including the
// on-screen console it is in the middle of building, so the failure reports
// nowhere at all. Push those to the backend, which prints to stderr, so a
// blank or half-built UI is diagnosable from a terminal instead of by guessing.
function reportToBackend(level, target, msg) {
  try { invoke("ui_log", { level, target, msg: String(msg) }); } catch (_) {}
}
window.addEventListener("error", (e) => {
  reportToBackend("error", "uncaught",
    `${e.message} at ${e.filename ?? "?"}:${e.lineno ?? "?"}:${e.colno ?? "?"}`);
});
window.addEventListener("unhandledrejection", (e) => {
  reportToBackend("error", "rejection", e.reason?.stack ?? e.reason);
});
reportToBackend("trace", "boot", "app.js evaluating");

// ---------------------------------------------------------------- console log

// System-console-style leveled log: HH:MM:SS.mmm LEVEL message, color-coded,
// filterable by level, repeats collapsed into a (xN) counter, and copyable to
// the clipboard as plain text -- this is the only visibility into the webview
// the app has, so it needs to survive real debugging, not just happy-path status.
const LEVELS = ["trace", "info", "warn", "error"];
const activeLevels = new Set(LEVELS);
let logBuf = []; // { t: Date, level, msg, n }
const LOG_CAP = 4000;

function pad2(n) { return String(n).padStart(2, "0"); }
function pad3(n) { return String(n).padStart(3, "0"); }
function ts(d) {
  return `${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}.${pad3(d.getMilliseconds())}`;
}
function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  }[c]));
}

function logAt(level, msg) {
  const last = logBuf[logBuf.length - 1];
  if (last && last.level === level && last.msg === msg) {
    last.n++;
    last.t = new Date();
  } else {
    logBuf.push({ t: new Date(), level, msg, n: 1 });
    if (logBuf.length > LOG_CAP) logBuf.shift();
  }
  renderLog();
}
const logTrace = (m) => logAt("trace", m);
const log = (m) => logAt("info", m);
const logWarn = (m) => logAt("warn", m);
const logError = (m) => logAt("error", m);

function renderLog() {
  const el = $("log");
  if (!el) return;
  const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
  el.innerHTML = logBuf
    .filter((e) => activeLevels.has(e.level))
    .map((e) => {
      const count = e.n > 1 ? `<span class="cnt">(x${e.n})</span>` : "";
      return `<div class="logline lvl-${e.level}"><span class="t">${ts(e.t)}</span>` +
        `<span class="lvl">${e.level.toUpperCase()}</span>` +
        `<span class="msg">${escapeHtml(e.msg)}</span>${count}</div>`;
    })
    .join("");
  if (nearBottom) el.scrollTop = el.scrollHeight;
}

document.querySelectorAll("#logbar .lvl").forEach((cb) => {
  cb.onchange = () => {
    logTrace(`click: level filter ${cb.value} -> ${cb.checked}`);
    cb.checked ? activeLevels.add(cb.value) : activeLevels.delete(cb.value);
    renderLog();
  };
});

$("logClear").onclick = () => {
  logBuf = [];
  renderLog();
};

function execCommandCopy(text) {
  // Fallback for webviews that expose the async Clipboard API but deny it
  // (e.g. no permission prompt available on file:// / custom-protocol origins):
  // a hidden textarea + the legacy execCommand path, which most webviews still honor.
  const ta = document.createElement("textarea");
  ta.value = text;
  ta.style.position = "fixed";
  ta.style.opacity = "0";
  document.body.appendChild(ta);
  ta.select();
  const ok = document.execCommand("copy");
  document.body.removeChild(ta);
  return ok ? Promise.resolve() : Promise.reject(new Error("execCommand('copy') failed"));
}

async function copyToClipboard(text) {
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return;
    } catch (e) {
      // fall through to execCommand
    }
  }
  return execCommandCopy(text);
}

$("logCopy").onclick = async () => {
  logTrace("click: Copy");
  const text = logBuf
    .map((e) => `${ts(e.t)} ${e.level.toUpperCase().padEnd(5)} ${e.msg}${e.n > 1 ? ` (x${e.n})` : ""}`)
    .join("\n");
  const btn = $("logCopy");
  try {
    await copyToClipboard(text);
    const prev = btn.textContent;
    btn.textContent = "Copied";
    setTimeout(() => (btn.textContent = prev), 1200);
  } catch (e) {
    logError(`clipboard: ${e}`);
  }
};

// Surface JS errors in the on-screen log: the webview has no visible console,
// and a throw partway down this file silently kills every handler below it.
window.addEventListener("error", (e) =>
  logError(`JS error: ${e.message} @ ${String(e.filename).split("/").pop()}:${e.lineno}`));
window.addEventListener("unhandledrejection", (e) => logError(`JS promise: ${e.reason}`));

// ---------------------------------------------------------------- backend log bridge

// The Rust backend emits a `backend-log` event for its own trace/info/warn/
// error activity -- HTTP requests, the Digest challenge/retry flow, UDP
// capture milestones, command entry/exit -- so it lands in this SAME console
// next to the frontend's own lines. See app/src-tauri/src/logging.rs.
if (window.__TAURI__.event?.listen) {
  window.__TAURI__.event.listen("backend-log", (event) => {
    const { level, target, msg } = event.payload;
    logAt(level, `[${target}] ${msg}`);
  });
  logTrace("backend log bridge connected");
} else {
  logWarn("backend log bridge unavailable -- window.__TAURI__.event not found");
}

log("ui: PFlowBench frontend ready");

// ---------------------------------------------------------------- settings

const addr = () => $("addr").value.trim();
const mode = () => $("mode").value;
const user = () => $("user").value.trim();
$("mode").onchange = () => {
  logTrace(`change: Auth mode -> ${mode()}`);
  $("userRow").style.display = mode() === "digest" ? "block" : "none";
};
localStorage.addr && ($("addr").value = localStorage.addr);
$("addr").onchange = () => {
  logTrace(`change: Address -> ${addr()}`);
  localStorage.addr = addr();
  detectMetricsHost();
};

// The metrics host is what the printer streams loadcell/Z telemetry TO, so a
// blank one silently produces a job that captures nothing. Detect it up front
// rather than making it depend on someone having pressed Test in another card.
async function detectMetricsHost() {
  try {
    const ip = await invoke("local_ip", { target: addr() });
    $("ipHint").textContent = ip;
    if (!$("mhost").value.trim()) {
      $("mhost").value = ip;
      logTrace(`metrics host auto-detected: ${ip}`);
    }
  } catch (e) {
    logTrace(`metrics host detection: ${e}`);
  }
  markMetricsHost();
}

// Make an empty metrics host impossible to miss -- it is the single field
// whose absence wastes a whole print.
function markMetricsHost() {
  const el = $("mhost");
  const empty = !el.value.trim();
  el.style.borderColor = empty ? "var(--bad)" : "";
  $("genBtn").title = empty ? "set a metrics host first" : "";
}
$("mhost").oninput = markMetricsHost;
detectMetricsHost();

async function refreshKeyState() {
  const has = await invoke("key_status").catch(() => false);
  logTrace(`key_status -> ${has}`);
  $("keyState").textContent = has ? "keychain: stored" : "keychain: empty";
  $("keyState").style.color = has ? "var(--good)" : "var(--warn)";
}
refreshKeyState();

$("saveKey").onclick = async () => {
  logTrace("click: Save key");
  const s = $("secret").value.trim();
  if (!s) return logWarn("save key: nothing to save -- the field was empty");
  if (s !== $("secret").value) logWarn("save key: stripped surrounding whitespace from the pasted value");
  logTrace(`save key: ${s.length} chars, mode=${mode()}`);
  try {
    await invoke("key_save", { secret: s });
    $("secret").value = ""; // never keep it in the DOM
    log("credential saved and read back from OS keychain");
  } catch (e) {
    logError(`keychain: ${e}`);
  }
  refreshKeyState();
};

$("testBtn").onclick = async () => {
  logTrace("click: Test");
  logTrace(`GET /api/v1/info -- addr=${addr()} mode=${mode()}${mode() === "digest" ? ` user=${user()}` : ""}`);
  try {
    const body = await invoke("printer_info", { addr: addr(), mode: mode(), user: user() });
    const j = JSON.parse(body);
    $("conn").textContent = `printer: ${j.name ?? j.hostname ?? "ok"}`;
    $("conn").className = "ok";
    log(`connected: ${body.slice(0, 120)}`);
    detectMetricsHost();
    // A successful connection test is a nice moment to confirm audibly --
    // the backend skips this itself if the printer is mid-job.
    try {
      const b = await invoke("printer_beep", { addr: addr(), mode: mode(), user: user() });
      logTrace(`beep: ${b}`);
    } catch (e) {
      logTrace(`beep failed (non-fatal): ${e}`);
    }
  } catch (e) {
    $("conn").textContent = "printer: error";
    $("conn").className = "err";
    logError(`connect failed: ${e}`);
    if (String(e).includes("401")) {
      logWarn(`a 401 means the printer rejected the credential for auth mode "${mode()}" -- ` +
        `double-check the mode dropdown matches what was saved (API key vs Digest user/password), ` +
        `and that the key wasn't rotated on the touchscreen since the last Save.`);
    }
  }
};

// A profile names the whole parameter set for a material+nozzle, so switching
// material is one selection instead of a dozen fields that can be half-changed.
async function loadProfiles() {
  try {
    const ps = await invoke("profiles_list");
    const sel = $("profileSel");
    for (const p of ps) {
      const o = document.createElement("option");
      o.value = p.path;
      o.textContent = p.error ? `${p.name} (INVALID)` : p.name;
      sel.appendChild(o);
    }
    log(`profiles: ${ps.length} found`);
    ps.filter((p) => p.error).forEach((p) => logError(`profile ${p.path}: ${p.error}`));
  } catch (e) {
    logTrace(`profiles: ${e}`);
  }
}

$("profileSel").onchange = async () => {
  const sel = $("profileSel");
  logTrace(`change: profile -> ${sel.value || "(defaults)"}`);
  const info = $("profileInfo");
  if (!sel.value) { info.textContent = ""; return; }
  try {
    const p = (await invoke("profiles_list")).find((x) => x.path === sel.value);
    if (!p) return;

    // Fill the form FROM the profile. Leaving the previous material's ladder
    // sitting in the box is how a PC run nearly went out with PETG numbers:
    // the field is passed as an override, so what is displayed must be what
    // actually runs.
    $("flows").value = p.flows.join(", ");
    if ($("revs")) $("revs").value = p.revs;
    $("layerh").value = p.layerH;
    $("flh").value = p.firstLayerH;
    log(`profile '${p.name}': flows ${p.flows.join(", ")} mm³/s, ${p.temps.join("/")} °C, bed ${p.bed} °C, fan ${p.fan}, brim ${p.brim}, em ${p.em}`);
    log(`profile reference: ${p.refDesc}`);
    if (p.published) {
      const pct = Math.round((p.flowHi / p.published) * 100);
      log(`vendor publishes ${p.published} mm³/s; this ladder tops out at ${p.flowHi} (${pct}%)`);
    }

    const mismatch = (p.lint ?? []).filter((l) => l.startsWith("REFERENCE MISMATCH"));
    mismatch.forEach((m) => logError(m));
    (p.lint ?? []).filter((l) => !l.startsWith("REFERENCE MISMATCH")).forEach((l) => logWarn(`profile: ${l}`));

    // The card shows the numbers that drive the run, one line each. The
    // profile's research notes (sources, measurements, reasoning) stay in the
    // file where they belong -- here they are one click away, never a wall of
    // text over the form.
    info.innerHTML =
      `<b>${p.temps.join(" / ")} °C</b> &middot; flow ${p.flowLo}–${p.flowHi} mm³/s ` +
      `&middot; bed ${p.bed} °C &middot; ${p.nozzle} mm &middot; fan ${p.fan} &middot; brim ${p.brim}<br>` +
      `<b>reference:</b> ${p.refDesc}` +
      (p.lint?.length
        ? `<br><span style="color:${mismatch.length ? "var(--err)" : "var(--warn)"}">${p.lint.join("<br>")}</span>`
        : "") +
      (p.notes?.length
        ? `<details style="margin-top:4px"><summary style="cursor:pointer;color:var(--dim)">` +
          `research notes (${p.notes.length})</summary>` +
          `<div style="opacity:.8;max-height:180px;overflow-y:auto;margin-top:4px">` +
          p.notes.map((n) => `<div style="margin-bottom:3px">${n}</div>`).join("") +
          `</div></details>`
        : "");
  } catch (e) {
    logWarn(`profile info: ${e}`);
  }
};
loadProfiles();
reportToBackend("trace", "boot", "app.js top-level init complete");

$("genBtn").onclick = async () => {
  logTrace("click: Generate G-code");
  const cfg = {
    reference: $("refpath").value.trim(),
    out: $("gpath").value.trim(),
    addr: addr(),
    profile: $("profileSel").value,
    metricsHost: $("mhost").value.trim(),
    metricsPort: parseInt($("port").value, 10),
    temps: "",
    flows: $("flows").value.trim(),
    bedW: 300, bedH: 300,
  };
  logTrace(`gcode_generate: ${JSON.stringify(cfg)}`);
  try {
    const r = await invoke("gcode_generate", cfg);
    log(`generated: ${r}`);
  } catch (e) {
    logError(`generate failed: ${e}`);
  }
};

$("uploadBtn").onclick = async () => {
  logTrace("click: Upload only");
  const p = $("gpath").value.trim();
  if (!p) return logWarn("upload: set the G-code file path first");
  const storage = $("storage").value.trim();
  const name = $("gname").value.trim();
  logTrace(`PUT /api/v1/files/${storage}/${name} <- ${p}`);
  try {
    const r = await invoke("printer_upload", {
      addr: addr(), mode: mode(), user: user(),
      storage, name, gcodePath: p, printAfter: false,
    });
    log(`upload: ${r}`);
  } catch (e) {
    logError(`upload failed: ${e}`);
  }
};

// ---------------------------------------------------------------- camera

// The camera is a separate WiFi device (Buddy3D) that only opens its RTSP
// server while local streaming is enabled in the Prusa App -- the backend
// probes for that and returns an actionable message, so failures here are
// surfaced rather than swallowed.
let camTimer = null;
let camStatsTimer = null;
let camSeq = 0;

function setCamState(text, cls) {
  const el = $("camState");
  el.textContent = `camera: ${text}`;
  el.className = cls || "";
}

function renderCamInfo(rows) {
  $("camInfo").innerHTML = rows
    .filter(([, v]) => v !== null && v !== undefined && v !== "")
    .map(([k, v]) => `<tr><td>${k}</td><td>${v}</td></tr>`)
    .join("");
}

const fmtBytes = (b) =>
  b > 1048576 ? `${(b / 1048576).toFixed(1)} MB` : `${(b / 1024).toFixed(0)} KB`;

// Stream details for the curious: everything the camera advertises plus live
// throughput measured from the frames actually arriving.
async function pollCamStats() {
  try {
    const s = await invoke("camera_stats");
    if (!s.connected) return;
    renderCamInfo([
      ["state", s.live ? "live (all frames)" : "keyframes only"],
      ["frames decoded", s.frames],
      ["last frame", fmtBytes(s.lastFrameBytes)],
      ["received", fmtBytes(s.bytes)],
      ["bitrate", s.kbps ? `${(s.kbps / 1000).toFixed(2)} Mbps` : null],
      ["frame interval", s.keyframeIntervalS ? `${(s.keyframeIntervalS * 1000).toFixed(0)} ms` : null],
      ["effective fps", s.keyframeIntervalS ? (1 / s.keyframeIntervalS).toFixed(1) : null],
      ["uptime", `${s.uptimeS.toFixed(0)} s`],
      ["recording", s.recording ? `yes — ${s.recordedFrames} frames` : "no"],
      ["decode failures", s.decodeFailures],
    ]);
    $("camLapseBtn").disabled = s.recordedFrames === 0;
    // reflect recording state driven from anywhere (a capture run starts it too)
    camRecording = s.recording;
    $("camRecBtn").textContent = s.recording
      ? `■ Stop (${s.recordedFrames})`
      : s.recordedFrames ? `● Record (${s.recordedFrames} held)` : "● Record";
    $("camRecBtn").style.color = s.recording ? "var(--bad)" : "";
    $("camRecBtn").style.borderColor = s.recording ? "var(--bad)" : "";
  } catch (e) {
    logTrace(`camera stats: ${e}`);
  }
}

// Copy the frame on screen to the clipboard. Modern webviews want a real
// image Blob via ClipboardItem; writing the data URL as text would paste a
// wall of base64 instead of a picture.
$("camCopyBtn").onclick = async () => {
  logTrace("click: Copy image");
  const src = $("camImg").src;
  if (!src || !src.startsWith("data:image")) return logWarn("camera: no frame to copy yet");
  const btn = $("camCopyBtn");
  try {
    const blob = await (await fetch(src)).blob();
    await navigator.clipboard.write([new ClipboardItem({ [blob.type]: blob })]);
    const prev = btn.textContent;
    btn.textContent = "Copied";
    setTimeout(() => (btn.textContent = prev), 1200);
    log(`camera: frame copied to clipboard (${(blob.size / 1024).toFixed(0)} KB)`);
  } catch (e) {
    // Clipboard image writes are commonly blocked outside a trusted context.
    logError(`copy image failed: ${e}`);
  }
};

let camRecording = false;

$("camRecBtn").onclick = async () => {
  const on = !camRecording;
  logTrace(`click: Record -> ${on}`);
  try {
    const held = await invoke("camera_record", { on });
    log(on ? "camera: recording started (previous frames discarded)"
           : `camera: recording stopped, ${held} frames held`);
    await pollCamStats();
  } catch (e) {
    logError(`camera record: ${e}`);
  }
};

$("camTestBtn").onclick = async () => {
  logTrace("click: Test camera");
  const host = $("camhost").value.trim();
  if (!host) return logWarn("camera: set the camera address first");
  try {
    const i = await invoke("camera_test", { host });
    log(`camera OK: ${i.encoding} ${i.width}x${i.height} ${i.profile ?? ""} ${i.level ? "L" + i.level.toFixed(1) : ""}`.trim());
    renderCamInfo([
      ["url", i.url],
      ["server", i.server],
      ["codec", `${i.encoding} ${i.profile ?? ""} ${i.level ? "L" + i.level.toFixed(1) : ""}`.trim()],
      ["resolution", i.width && i.height ? `${i.width} × ${i.height}` : null],
      ["clock rate", i.clockRate ? `${(i.clockRate / 1000).toFixed(0)} kHz` : null],
      ["payload type", i.payloadType],
      ["transport", i.transport],
      ["methods", (i.methods || []).join(", ")],
    ]);
    setCamState("reachable", "live");
  } catch (e) {
    logError(`camera test failed: ${e}`);
    setCamState("unreachable", "err");
    renderCamInfo([["error", String(e)]]);
  }
};

$("camLive").onchange = async () => {
  const live = $("camLive").checked;
  logTrace(`click: full-rate live -> ${live}`);
  try {
    await invoke("camera_set_live", { live });
    // repaint faster when every frame counts, slower when it's keyframe-only
    clearInterval(camTimer);
    camTimer = setInterval(pollCamera, live ? 60 : 1000);
  } catch (e) {
    logWarn(`camera: ${e}`);
  }
};

$("camLapseBtn").onclick = async () => {
  logTrace("click: Export MP4");
  try {
    const r = await invoke("camera_write_timelapse", { fps: 25 });
    log(`timelapse: ${r}`);
  } catch (e) {
    logError(`timelapse failed: ${e}`);
  }
};

// ------------------------------------------------------------ vision

// Calibration: four clicks on the live view, one per test object brim centre in
// PRINT ORDER (the order the summary lists the temperatures). Click
// coordinates are scaled from the displayed size to the camera's native
// pixels before they reach the backend.
let visionClicks = null;

$("visionCalBtn").onclick = () => {
  logTrace("click: Calibrate vision");
  if (visionClicks) {
    visionClicks = null;
    $("visionState").textContent = "";
    $("camImg").style.cursor = "";
    log("vision: calibration cancelled");
    return;
  }
  visionClicks = [];
  $("camImg").style.cursor = "crosshair";
  $("visionState").textContent = "click brim centre 1 of 4 (print order)";
  log("vision: click the 4 brim centres in print order on the live view");
};

$("camImg").addEventListener("click", async (ev) => {
  if (!visionClicks) return;
  const img = $("camImg");
  const r = img.getBoundingClientRect();
  // #camImg uses object-fit: contain -- the frame is centered inside the
  // element with letterbox bars. Map through the rendered rectangle, not
  // the element box, and ignore clicks on the bars.
  const scale = Math.min(r.width / img.naturalWidth, r.height / img.naturalHeight);
  const rw = img.naturalWidth * scale;
  const rh = img.naturalHeight * scale;
  const ox = r.left + (r.width - rw) / 2;
  const oy = r.top + (r.height - rh) / 2;
  const x = (ev.clientX - ox) / scale;
  const y = (ev.clientY - oy) / scale;
  if (x < 0 || y < 0 || x > img.naturalWidth || y > img.naturalHeight) {
    logWarn("vision: click landed on the letterbox, not the frame -- try again");
    return;
  }
  visionClicks.push([x, y]);
  logTrace(`vision: click ${visionClicks.length} at ${x.toFixed(0)},${y.toFixed(0)}`);
  if (visionClicks.length < 4) {
    $("visionState").textContent = `click brim centre ${visionClicks.length + 1} of 4`;
    return;
  }
  const clicks = visionClicks;
  visionClicks = null;
  img.style.cursor = "";
  $("visionState").textContent = "";
  try {
    const r2 = await invoke("vision_calibrate", { clicks });
    log(`vision: ${r2}`);
  } catch (e) {
    logError(`vision calibrate: ${e}`);
  }
});

$("visionRunBtn").onclick = async () => {
  logTrace("click: Analyze run");
  try {
    const r = await invoke("vision_analyze", { dir: "" });
    const votes = { Grow: 0, Marginal: 0, Stall: 0, NoVote: 0 };
    for (const b of r.bands) {
      votes[b.vote] = (votes[b.vote] || 0) + 1;
      const line =
        `vision: cyl${b.cylinder} band ${b.band} @ ${b.flow.toFixed(1)} ` +
        `-> ${b.vote}${b.usable ? "" : " (no vote)"} -- ${b.note}`;
      if (b.vote === "Stall") logWarn(line);
      else logTrace(line);
    }
    log(
      `vision: ${r.bands.length} bands -- ${votes.Grow} grow, ` +
      `${votes.Marginal} marginal, ${votes.Stall} STALL, ` +
      `${votes.NoVote} no vote (${r.dir})`
    );
  } catch (e) {
    logError(`vision analyze: ${e}`);
  }
};

async function pollCamera() {
  try {
    const p = await invoke("camera_preview", { sinceSeq: camSeq });
    if (p.image) {
      camSeq = p.seq;
      $("camImg").src = p.image;
      $("camImg").classList.add("on");
      $("camHint").style.display = "none";
      $("camDot").classList.add("on");
      setCamState(`live (frame ${p.seq})`, "live");
    } else if (!p.connected) {
      setCamState("waiting for first frame", "");
    }
  } catch (e) {
    logTrace(`camera poll: ${e}`);
  }
}

$("camStartBtn").onclick = async () => {
  logTrace("click: Connect camera");
  const host = $("camhost").value.trim();
  if (!host) return logWarn("camera: set the camera address first");
  try {
    await invoke("camera_start", { host });
    log(`camera: connected to ${host}`);
    $("camStartBtn").disabled = true;
    $("camStopBtn").disabled = false;
    $("camHint").textContent = "connecting, first frame takes ~3s";
    setCamState("connecting", "");
    camSeq = 0;
    clearInterval(camTimer);
    // ~1s poll against a ~3s keyframe cadence; the backend returns no image
    // when nothing changed, so idle polls are cheap.
    camTimer = setInterval(pollCamera, $("camLive").checked ? 60 : 1000);
    camStatsTimer = setInterval(pollCamStats, 1000);
    $("camCopyBtn").disabled = false;
    $("camRecBtn").disabled = false;
    $("visionCalBtn").disabled = false;
  } catch (e) {
    logError(`camera: ${e}`);
    setCamState("error", "err");
    $("camHint").textContent = String(e);
  }
};

$("camStopBtn").onclick = async () => {
  logTrace("click: Disconnect camera");
  clearInterval(camTimer);
  clearInterval(camStatsTimer);
  camTimer = null;
  camStatsTimer = null;
  await invoke("camera_stop").catch((e) => logWarn(`camera stop: ${e}`));
  $("camStartBtn").disabled = false;
  $("camStopBtn").disabled = true;
  $("camImg").classList.remove("on");
  $("camDot").classList.remove("on");
  $("camHint").style.display = "";
  $("camHint").textContent = "not connected";
  $("camLapseBtn").disabled = true;
  $("camCopyBtn").disabled = true;
  $("camRecBtn").disabled = true;
  $("visionCalBtn").disabled = true;
  // a half-done calibration cannot continue without the live view
  visionClicks = null;
  $("visionState").textContent = "";
  $("camImg").style.cursor = "";
  setCamState("off", "");
  log("camera: disconnected");
};

// ------------------------------------------------------- snapshot strip

// The backend pushes one of these as each flow band completes. Each thumbnail
// is the visual evidence for exactly one commanded flow rate, which is the
// whole point -- it lets a failure be traced back to the band that caused it.
function addBandSnapshot(s) {
  const el = document.createElement("div");
  el.className = "snap";
  el.innerHTML =
    `<img src="${s.image}" alt="cylinder ${s.cylinder} band ${s.band}">` +
    `<div class="cap"><b>band ${s.band}</b><span>${s.flow.toFixed(1)} mm³/s</span></div>`;
  el.onclick = () => {
    $("lightboxImg").src = s.image;
    $("lightboxCap").textContent =
      `cylinder ${s.cylinder} · band ${s.band} · ${s.flow.toFixed(1)} mm³/s · Z ${s.z.toFixed(2)}mm — ${s.file}`;
    $("lightbox").classList.add("on");
  };
  const strip = $("snapStrip");
  strip.appendChild(el);
  // Scroll the strip itself, NOT via scrollIntoView: that walks every
  // scrollable ancestor and drags the whole page sideways, pushing the
  // sidebar off-screen as bands accumulate.
  strip.scrollTo({ left: strip.scrollWidth, behavior: "smooth" });
}

$("lightbox").onclick = () => $("lightbox").classList.remove("on");
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") $("lightbox").classList.remove("on");
});

if (window.__TAURI__.event?.listen) {
  window.__TAURI__.event.listen("band-snapshot", (event) => {
    const s = event.payload;
    log(`band ${s.band} @ ${s.flow.toFixed(1)} mm³/s captured`);
    addBandSnapshot(s);
  });
}

// ---------------------------------------------------------------- plots

const css = (v) => getComputedStyle(document.documentElement).getPropertyValue(v).trim();
const W = () => $("tplot").clientWidth || 800;

const tplot = new uPlot(
  {
    width: W(), height: 300,
    series: [
      {},
      { label: "force (g)", stroke: css("--accent"), width: 1 },
      { label: "Z (mm)", stroke: css("--warn"), width: 1, scale: "z" },
    ],
    axes: [
      { stroke: css("--dim"), grid: { stroke: css("--line") } },
      { stroke: css("--accent"), grid: { stroke: css("--line") } },
      { scale: "z", side: 1, stroke: css("--warn"), grid: { show: false } },
    ],
    scales: { x: { time: false } },
    legend: { show: true },
  },
  [[], [], []],
  $("tplot")
);

// The knee plot carries ONE CURVE PER CYLINDER. It used to draw only the
// highest cylinder index, which meant a four-temperature run showed a single
// temperature -- and, because the cylinder counter also fires on travel and
// purge moves, the chart kept resetting to whatever artifact had just started
// and read as empty. Every cylinder is now its own series, so all four
// test objects are comparable side by side and the artifact ones are visibly
// flat instead of masquerading as the result.
const KNEE_COLORS = [
  "#4ea1ff", "#48d597", "#f5a524", "#f2557a",
  "#a78bfa", "#22d3ee", "#facc15", "#fb7185",
  "#818cf8", "#34d399", "#fbbf24", "#f87171",
];

let kplot = null;
let kplotKey = null; // what the current instance was built for

// Cylinders differ in magnitude by two orders: a real test object ramps to
// ~14000 g, while an artifact cylinder (the counter also fires on travel and
// purge moves) never leaves -100..+300. On one shared axis the small ones are
// pinned flat against the bottom and unreadable. Anything peaking below this
// fraction of the run's largest peak gets its own right-hand axis instead of
// being squashed against the big ones.
const KNEE_MINOR_FRACTION = 0.2;

function makeKnee(cyls, scales) {
  if (kplot) kplot.destroy();
  kplotKey = cyls.join(",") + "|" + scales.join(",");
  kplot = new uPlot(
    {
      width: W(), height: 260,
      series: [
        { label: "flow (mm³/s)" },
        ...cyls.map((cy, i) => ({
          label: scales[i] === "lo" ? `cyl ${cy} (right)` : `cyl ${cy}`,
          stroke: KNEE_COLORS[i % KNEE_COLORS.length],
          width: scales[i] === "lo" ? 1 : 2,
          dash: scales[i] === "lo" ? [4, 3] : undefined,
          scale: scales[i],
          points: { show: true, size: 6 },
          spanGaps: false,
        })),
      ],
      axes: [
        { stroke: css("--dim"), grid: { stroke: css("--line") } },
        { scale: "y", stroke: css("--good"), grid: { stroke: css("--line") },
          label: "mean force (g)" },
        // Only drawn when something is actually assigned to it.
        { scale: "lo", side: 1, stroke: css("--dim"), grid: { show: false },
          label: "low-range (g)", show: scales.includes("lo") },
      ],
      scales: { x: { time: false } },
      legend: { show: true },
    },
    [[], ...cyls.map(() => [])],
    $("kplot")
  );
  return kplot;
}
makeKnee([], []);

// Third chart: speed and chamber temperature share a time axis with the
// force plot, so a knee can be checked against both -- a drop in speed means
// the printer never reached the commanded feedrate, and a chamber excursion
// is an alternative explanation for a change in back-pressure.
const splot = new uPlot(
  {
    width: W(), height: 220,
    series: [
      {},
      { label: "speed (mm/s)", stroke: css("--accent"), width: 1 },
      { label: "chamber (°C)", stroke: css("--warn"), width: 1.5, scale: "c" },
    ],
    axes: [
      { stroke: css("--dim"), grid: { stroke: css("--line") } },
      { stroke: css("--accent"), grid: { stroke: css("--line") } },
      { scale: "c", side: 1, stroke: css("--warn"), grid: { show: false } },
    ],
    scales: { x: { time: false } },
    legend: { show: true },
  },
  [[], [], []],
  $("splot")
);

// ---------------------------------------------------------------- time window
//
// One shared viewing window for BOTH time charts (force/Z and speed/chamber),
// so their x axes always match and a long run stops compressing into an
// unreadable smear. Data is never discarded -- the window only sets the
// visible x range; uPlot re-autoranges y over what's visible.
let viewSpan = 300;        // seconds; Infinity = whole run
let viewEnd = null;        // null = follow the newest sample ("Live")
let tMaxSeen = 0;          // newest timestamp across all series

function fmtClock(sec) {
  const m = Math.floor(sec / 60), s = Math.floor(sec % 60);
  return `${m}:${String(s).padStart(2, "0")}`;
}

function applyTimeWindow() {
  const end = viewEnd == null ? tMaxSeen : viewEnd;
  const span = viewSpan === Infinity ? Math.max(tMaxSeen, 1) : viewSpan;
  const min = Math.max(0, end - span);
  const max = Math.max(min + 1, end);
  tplot.setScale("x", { min, max });
  splot.setScale("x", { min, max });
  $("navPos").textContent =
    (viewEnd == null ? "live " : "") + `${fmtClock(min)}–${fmtClock(max)}`;
  $("navLive").style.fontWeight = viewEnd == null ? "bold" : "normal";
}

$("navSpan").onchange = () => {
  const v = $("navSpan").value;
  viewSpan = v === "all" ? Infinity : parseInt(v, 10);
  logTrace(`chart window: span -> ${v}`);
  applyTimeWindow();
};
$("navStart").onclick = () => {
  viewEnd = viewSpan === Infinity ? tMaxSeen : Math.min(viewSpan, tMaxSeen);
  logTrace("chart window: jump to start");
  applyTimeWindow();
};
$("navBack").onclick = () => {
  const step = (viewSpan === Infinity ? tMaxSeen : viewSpan) / 2;
  viewEnd = Math.max(viewSpan === Infinity ? tMaxSeen : viewSpan,
                     (viewEnd == null ? tMaxSeen : viewEnd) - step);
  logTrace("chart window: back");
  applyTimeWindow();
};
$("navFwd").onclick = () => {
  const step = (viewSpan === Infinity ? tMaxSeen : viewSpan) / 2;
  const next = (viewEnd == null ? tMaxSeen : viewEnd) + step;
  // walking forward past the newest data flips back into follow mode
  viewEnd = next >= tMaxSeen ? null : next;
  logTrace("chart window: forward");
  applyTimeWindow();
};
$("navLive").onclick = () => {
  viewEnd = null;
  logTrace("chart window: live");
  applyTimeWindow();
};

window.addEventListener("resize", () => {
  tplot.setSize({ width: W(), height: 300 });
  if (kplot) kplot.setSize({ width: W(), height: 260 });
  splot.setSize({ width: W(), height: 220 });
});

// ---------------------------------------------------------------- capture

let seq = 0;
let capTimer = null;
let sawPrinting = false;
let statusTimer = null;
let tF = [], vF = [], tZ = [], vZ = [];

// uPlot wants ONE x array shared by every series on a chart. Resampling one
// series onto another's timestamps makes it a hostage: if the base series has
// no samples, the x array is empty and every other series vanishes with it --
// which is exactly what hid a climbing chamber temperature behind a stationary
// print head, since the head emits no speed samples until the job starts
// moving. Build the axis as the UNION of every series' own timestamps instead,
// so each one plots on the clock it was actually sampled on.
//
// Both inputs are already sorted, so this is a linear k-way merge rather than
// a sort -- it runs on every 200ms poll against arrays that reach six figures
// over a long run.
function unionSeries(series) {
  const idx = series.map(() => 0);
  const x = [];
  for (;;) {
    let next = Infinity;
    for (let k = 0; k < series.length; k++) {
      const i = idx[k];
      if (i < series[k].length && series[k][i][0] < next) next = series[k][i][0];
    }
    if (next === Infinity) break;
    x.push(next);
    for (let k = 0; k < series.length; k++) {
      while (idx[k] < series[k].length && series[k][idx[k]][0] === next) idx[k]++;
    }
  }

  // Step-sample each series onto the union: hold the last value at or before
  // each tick, and stay null before the series' first real sample rather than
  // back-filling a value that was never measured.
  const cols = series.map((s) => {
    const col = new Array(x.length).fill(null);
    let j = 0;
    for (let i = 0; i < x.length && s.length; i++) {
      while (j + 1 < s.length && s[j + 1][0] <= x[i]) j++;
      if (s[j][0] <= x[i]) col[i] = s[j][1];
    }
    return col;
  });
  return [x, ...cols];
}

function mergedX() {
  const pairF = tF.map((t, i) => [t, vF[i]]);
  const pairZ = tZ.map((t, i) => [t, vZ[i]]);
  return unionSeries([pairF, pairZ]);
}

async function pollCapture() {
  try {
    const d = await invoke("capture_delta", { since: seq });
    if (d.seq !== seq) {
      seq = d.seq;
      tF = d.force.map((p) => p[0]);
      vF = d.force.map((p) => p[1]);
      tZ = d.z.map((p) => p[0]);
      vZ = d.z.map((p) => p[1]);
      tplot.setData(mergedX());

      // Speed and chamber temperature are sampled on completely different
      // clocks -- speed only exists while the head is moving, chamber climbs
      // for many minutes before the first move. Each gets its own timestamps.
      const sp = d.speed || [], cham = d.temp_chamber || [];
      splot.setData(unionSeries([sp, cham]));

      // shared, non-compressing time window across both charts
      const lastOf = (a) => (a.length ? a[a.length - 1][0] : 0);
      tMaxSeen = Math.max(
        tMaxSeen,
        tF.length ? tF[tF.length - 1] : 0,
        tZ.length ? tZ[tZ.length - 1] : 0,
        lastOf(sp),
        lastOf(cham)
      );
      applyTimeWindow();

      // Live readout straight from the metrics stream. Test against null, not
      // truthiness: a legitimate reading of 0 (a stationary head, an unheated
      // chamber) is data, and a falsy check silently drops it.
      const num = (v) => typeof v === "number" && !Number.isNaN(v);
      // Nozzle and bed belong to pollPrinter: it is the only source carrying
      // the TARGET as well as the actual, and two writers on different clocks
      // made the readout flip format twice a second. Chamber stays here --
      // PrusaLink does not report it at all, the metrics stream does.
      if (num(d.now_chamber)) $("pcham").textContent = `${d.now_chamber.toFixed(1)} °C`;
      // No speed samples at all means the head has not moved yet -- say so,
      // rather than leaving a blank that reads as a broken metric.
      $("pspeed").textContent = sp.length
        ? `${sp[sp.length - 1][1].toFixed(1)} mm/s`
        : "not moving yet";

      $("pn").textContent = String(tF.length + tZ.length);
      $("pz").textContent = d.z_now.toFixed(2) + " mm";
      $("pcyl").textContent = String(d.cyl);
      renderBands(d.bands);
    }
  } catch (e) {
    // Expected during the brief window between Stop clearing the timer and the
    // backend actually tearing down the capture; deduped so it can't flood.
    logTrace(`capture poll: ${e}`);
  }
}

function renderBands(bands) {
  const tb = $("btab").querySelector("tbody");
  tb.innerHTML = "";
  const byCyl = new Map();
  for (const b of bands) {
    if (!byCyl.has(b.cylinder)) byCyl.set(b.cylinder, []);
    byCyl.get(b.cylinder).push(b);
  }
  // knee plot: every cylinder, on the shared flow ladder
  const cyls = [...byCyl.keys()].sort((a, b) => a - b);
  if (cyls.length) {
    // Split by magnitude, not by guessing which cylinders are "real": a
    // cylinder whose peak is a small fraction of the run's peak goes on the
    // secondary axis so it stays readable instead of flat-lining.
    const peak = (cy) => Math.max(...byCyl.get(cy).map((b) => Math.abs(b.mean)));
    const top = Math.max(...cyls.map(peak), 1);
    const scales = cyls.map((cy) => (peak(cy) < KNEE_MINOR_FRACTION * top ? "lo" : "y"));

    const key = cyls.join(",") + "|" + scales.join(",");
    if (key !== kplotKey) makeKnee(cyls, scales);

    // Shared x axis = every commanded flow any cylinder recorded, ascending.
    const flows = [...new Set(bands.map((b) => b.flow))].sort((a, b) => a - b);
    const cols = cyls.map((cy) => {
      const byFlow = new Map(byCyl.get(cy).map((b) => [b.flow, b.mean]));
      return flows.map((f) => (byFlow.has(f) ? byFlow.get(f) : null));
    });
    kplot.setData([flows, ...cols]);
  }
  for (const cy of cyls) {
    const rows = byCyl.get(cy).sort((a, b) => a.band - b.band);
    const base = rows.length ? rows[0].mean : 0;
    // crude knee marker: first band whose delta-vs-previous more than doubles
    let kneeAt = -1;
    for (let i = 2; i < rows.length; i++) {
      const d1 = rows[i - 1].mean - rows[i - 2].mean;
      const d2 = rows[i].mean - rows[i - 1].mean;
      if (d1 > 0 && d2 > 2.5 * d1) { kneeAt = i; break; }
    }
    rows.forEach((b, i) => {
      const tr = document.createElement("tr");
      if (i === kneeAt) tr.className = "knee";
      tr.innerHTML =
        `<td>${b.cylinder}</td><td>${b.band}</td><td>${b.flow.toFixed(1)}</td>` +
        `<td>${b.actual_flow != null ? b.actual_flow.toFixed(1) : "-"}</td>` +
        `<td>${b.speed != null ? b.speed.toFixed(1) : "-"}</td>` +
        `<td>${b.n}</td><td>${b.mean.toFixed(3)}</td><td>${b.sd.toFixed(3)}</td>` +
        `<td>${(b.mean - base >= 0 ? "+" : "")}${(b.mean - base).toFixed(3)}</td>`;
      tb.appendChild(tr);
    });
  }
}

async function pollPrinter() {
  try {
    const body = await invoke("printer_status", { addr: addr(), mode: mode(), user: user() });
    const j = JSON.parse(body);
    const p = j.printer ?? {};
    $("pstate").textContent = p.state ?? "-";

    // Finalize the run by itself: a print can end at 2am. Only after the job
    // was actually seen PRINTING (so a pre-print IDLE doesn't trigger), and
    // NOT on ATTENTION/PAUSED -- those can resume, and stopping there would
    // truncate a recoverable run.
    if (capTimer) {
      const st = p.state ?? "";
      if (st === "PRINTING") sawPrinting = true;
      else if (sawPrinting && ["FINISHED", "STOPPED", "IDLE", "READY"].includes(st)) {
        log(`printer reports ${st} -- finalizing the run automatically`);
        await stopCapture(`job ${st.toLowerCase()}`);
      }
    }
    $("pnoz").textContent = p.temp_nozzle != null ? `${p.temp_nozzle.toFixed(1)} / ${p.target_nozzle ?? "-"} °C` : "-";
    $("pbed").textContent = p.temp_bed != null ? `${p.temp_bed.toFixed(1)} / ${p.target_bed ?? "-"} °C` : "-";
  } catch (e) {
    $("pstate").textContent = "unreachable";
    logWarn(`printer status poll: ${e}`);
  }
}

$("startBtn").onclick = async () => {
  logTrace("click: Start capture");
  const flows = $("flows").value.split(",").map((s) => parseFloat(s.trim())).filter((x) => !isNaN(x));
  const cfg = {
    port: parseInt($("port").value, 10),
    flows,
    revs: parseInt($("revs").value, 10),
    layerH: parseFloat($("layerh").value),
    firstLayerH: parseFloat($("flh").value),
    width: 0.9,
    // The generated file's manifest gives the capture deterministic sdpos
    // band addressing; without it the backend falls back to the Z heuristic
    // and says so in the console.
    gcodePath: $("gpath").value.trim(),
  };
  logTrace(`capture_start: ${JSON.stringify(cfg)}`);
  try {
    await invoke("capture_start", cfg);
    seq = 0;
    tMaxSeen = 0;
    viewEnd = null; // a new run starts in follow mode
    log(`capture listening on UDP :${$("port").value} - now start the print (or use Upload)`);
    $("startBtn").disabled = true;
    $("stopBtn").disabled = false;
    capTimer = setInterval(pollCapture, 200);
    statusTimer = setInterval(pollPrinter, 2000);
  } catch (e) {
    logError(`capture: ${e}`);
  }
};

$("exportBtn").onclick = async () => {
  logTrace("click: Export data");
  try {
    const r = await invoke("capture_export");
    log(`exported: ${r}`);
  } catch (e) {
    logError(`export failed: ${e}`);
  }
};

async function stopCapture(reason) {
  await invoke("capture_stop");
  clearInterval(capTimer);
  clearInterval(statusTimer);
  capTimer = null;
  statusTimer = null;
  sawPrinting = false;
  $("startBtn").disabled = false;
  $("stopBtn").disabled = true;
  log(`capture stopped (${reason}) -- run saved to the run directory`);
}

$("stopBtn").onclick = async () => {
  logTrace("click: Stop");
  await stopCapture("manual stop");
};

// ------------------------------------------------ camera calibration modal

// The camera model (full projection, from the pillar print) is solved once
// per camera setup and stored persistently; vision prefers it over the
// 4-click plane homography. The modal reports the stored state and runs the
// guided base/top click flow.
let calClicks = null;
let calPillars = [];

async function calRefreshState() {
  try {
    const s = await invoke("vision_camera_status");
    calPillars = s.pillars || [];
    $("calState").textContent = s.calibrated
      ? `calibrated -- ${s.info || "no details stored"}`
      : calPillars.length
        ? "not calibrated -- print generated, print it and start clicking"
        : "not calibrated -- generate and print the calibration object first";
    $("calClickBtn").disabled = !calPillars.length || !camTimer;
  } catch (e) {
    $("calState").textContent = String(e);
  }
}

$("calModalBtn").onclick = () => {
  logTrace("click: Camera calibration");
  $("calModal").style.display = "block";
  calRefreshState();
};
$("calCloseBtn").onclick = () => {
  $("calModal").style.display = "none";
};

$("calGenBtn").onclick = async () => {
  logTrace("click: Generate calibration G-code");
  const ref = $("refpath").value.trim();
  if (!ref) return logWarn("calibration: set the reference export path first");
  const out = ($("gpath").value.trim() || "flowcliff.gcode").replace(/\.gcode$/, "") + ".camcal.gcode";
  try {
    const r = await invoke("gcode_generate_calibration", {
      reference: ref,
      out,
      profile: $("profileSel").value,
      temps: "", // profile supplies temps, same as the main generate flow
    });
    log(`calibration: ${r}`);
    log(`calibration: upload and print ${out}, then reopen this dialog`);
    calRefreshState();
  } catch (e) {
    logError(`calibration generate: ${e}`);
  }
};

$("calClickBtn").onclick = () => {
  logTrace("click: Start calibration clicking");
  if (!calPillars.length) return;
  calClicks = [];
  $("calModal").style.display = "none"; // the live view needs to be visible
  $("camImg").style.cursor = "crosshair";
  const p = calPillars[0];
  $("visionState").textContent =
    `pillar 1 at (${p[0].toFixed(0)},${p[1].toFixed(0)}): click its BASE centre`;
  log("calibration: click base then top of each pillar, in the prompted order");
};

// shares the letterbox-aware coordinate mapping with the homography flow
$("camImg").addEventListener("click", async (ev) => {
  if (!calClicks) return;
  const img = $("camImg");
  const r = img.getBoundingClientRect();
  const scale = Math.min(r.width / img.naturalWidth, r.height / img.naturalHeight);
  const rw = img.naturalWidth * scale;
  const rh = img.naturalHeight * scale;
  const ox = r.left + (r.width - rw) / 2;
  const oy = r.top + (r.height - rh) / 2;
  const x = (ev.clientX - ox) / scale;
  const y = (ev.clientY - oy) / scale;
  if (x < 0 || y < 0 || x > img.naturalWidth || y > img.naturalHeight) {
    logWarn("calibration: click landed on the letterbox, not the frame");
    return;
  }
  calClicks.push([x, y]);
  const n = calClicks.length;
  const total = calPillars.length * 2;
  if (n < total) {
    const p = calPillars[Math.floor(n / 2)];
    const part = n % 2 === 0 ? "BASE" : "TOP";
    $("visionState").textContent =
      `pillar ${Math.floor(n / 2) + 1} at (${p[0].toFixed(0)},${p[1].toFixed(0)}): click its ${part} centre`;
    return;
  }
  const clicks = calClicks;
  calClicks = null;
  img.style.cursor = "";
  $("visionState").textContent = "";
  try {
    const res = await invoke("vision_camera_calibrate", { clicks });
    log(`calibration: ${res}`);
    $("calModal").style.display = "block";
    calRefreshState();
  } catch (e) {
    logError(`calibration: ${e}`);
    $("calModal").style.display = "block";
    calRefreshState();
  }
});
