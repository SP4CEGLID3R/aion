// Settings window: a form bound to RadarSettings by dotted field names, and a
// diagnostics panel polled from the backend.

import { backend } from "./lib/bridge";
import type { CaptureStatus, DataSourceConfig, DebugReport, RadarSettings, SourceAxis } from "./lib/types";

const form = document.querySelector<HTMLFormElement>("#form")!;
const warningsEl = document.querySelector<HTMLElement>("#warnings")!;
const AXES: SourceAxis[] = ["pos_x", "neg_x", "pos_y", "neg_y", "pos_z", "neg_z"];
const AXIS_LABEL: Record<SourceAxis, string> = {
  pos_x: "+X",
  neg_x: "−X",
  pos_y: "+Y",
  neg_y: "−Y",
  pos_z: "+Z",
  neg_z: "−Z",
};

let current: RadarSettings | null = null;

type Field = HTMLInputElement | HTMLSelectElement;

function fields(): Field[] {
  return [...form.querySelectorAll<Field>("input[name], select[name]")];
}

function getPath(obj: unknown, path: string): unknown {
  return path.split(".").reduce<unknown>((o, k) => (o as Record<string, unknown> | undefined)?.[k], obj);
}

function setPath(obj: Record<string, unknown>, path: string, value: unknown): void {
  const keys = path.split(".");
  const last = keys.pop()!;
  let o = obj;
  for (const k of keys) {
    o[k] = { ...((o[k] as Record<string, unknown>) ?? {}) };
    o = o[k] as Record<string, unknown>;
  }
  o[last] = value;
}

/** Colour inputs only accept #rrggbb. */
function sixDigit(hex: string): string {
  const h = hex.replace("#", "");
  if (h.length === 3) return `#${[...h].map((c) => c + c).join("")}`;
  return `#${h.slice(0, 6)}`;
}

function fill(s: RadarSettings): void {
  current = s;
  for (const el of fields()) {
    const v = getPath(s, el.name);
    if (el instanceof HTMLInputElement && el.type === "checkbox") el.checked = Boolean(v);
    else if (el instanceof HTMLInputElement && el.type === "color") el.value = sixDigit(String(v ?? "#000000"));
    else el.value = v === undefined || v === null ? "" : String(v);
  }
  showSourceSection(s.dataSource.kind);
}

function showSourceSection(kind: string): void {
  form.querySelectorAll<HTMLElement>("[data-source]").forEach((el) => {
    el.hidden = el.dataset.source !== kind;
  });
}

function read(): RadarSettings {
  const out = structuredClone(current!) as unknown as Record<string, unknown>;
  for (const el of fields()) {
    if (el.name.startsWith("dataSource.")) continue;
    let value: unknown;
    if (el instanceof HTMLInputElement && el.type === "checkbox") value = el.checked;
    else if (el instanceof HTMLInputElement && (el.type === "number" || el.type === "range")) value = Number(el.value);
    else value = el.value;
    setPath(out, el.name, value);
  }
  out.dataSource = readSource();
  return out as unknown as RadarSettings;
}

function readSource(): DataSourceConfig {
  const val = (name: string) => form.querySelector<HTMLInputElement>(`[name="dataSource.${name}"]`)!;
  const kind = (form.querySelector<HTMLSelectElement>('[name="dataSource.kind"]')!.value ??
    "none") as DataSourceConfig["kind"];
  switch (kind) {
    case "synthetic":
      return { kind, seed: Number(val("seed").value || 7), cubeCount: Number(val("cubeCount").value || 6) };
    case "replay":
      return { kind, path: val("path").value, speed: Number(val("speed").value || 1), loop: val("loop").checked };
    case "local_tcp":
      return { kind, port: Number(val("port").value || 47810) };
    case "packet_capture": {
      const text = (name: string) => {
        const v = form.querySelector<HTMLInputElement | HTMLSelectElement>(`[name="dataSource.${name}"]`)!.value.trim();
        return v === "" ? null : v;
      };
      return {
        kind,
        device: text("device"),
        file: text("file"),
        profilePath: text("profilePath"),
        record: val("record").checked,
      };
    }
    default:
      return { kind: "none" };
  }
}

function showWarnings(warnings: string[]): void {
  warningsEl.hidden = warnings.length === 0;
  warningsEl.replaceChildren();
  if (!warnings.length) return;
  const ul = document.createElement("ul");
  for (const w of warnings) {
    const li = document.createElement("li");
    li.textContent = w;
    ul.append(li);
  }
  warningsEl.append(ul);
}

function cell(text: string, cls?: string): HTMLTableCellElement {
  const td = document.createElement("td");
  td.textContent = text;
  if (cls) td.className = cls;
  return td;
}

function time(ms: number | null): string {
  return ms ? new Date(ms).toLocaleTimeString() : "—";
}

function renderDebug(r: DebugReport | null): void {
  const summary = document.querySelector<HTMLElement>("#debug-summary")!;
  const cubes = document.querySelector<HTMLTableSectionElement>("#debug-cubes tbody")!;
  const history = document.querySelector<HTMLTableSectionElement>("#debug-history tbody")!;
  if (!r) {
    summary.textContent = "No diagnostics in the browser preview.";
    return;
  }
  const d = r.diagnostics;
  const items: [string, string][] = [
    ["Source", `${r.source.name} — ${r.source.state}${r.source.detail ? ` (${r.source.detail})` : ""}`],
    ["Zone", r.zone ?? "—"],
    ["Accepted", String(d.accepted)],
    ["Duplicates", String(d.duplicates)],
    ["Adapter errors", String(r.adapterErrors)],
    ...Object.entries(d.rejected).map(([k, v]): [string, string] => [`Rejected: ${k}`, String(v)]),
    ...Object.entries(d.notLiveByProvenance).map(([k, v]): [string, string] => [`Not live: ${k}`, String(v)]),
  ];
  summary.replaceChildren(
    ...items.map(([k, v]) => {
      const div = document.createElement("div");
      const span = document.createElement("span");
      span.textContent = `${k}: `;
      div.append(span, v);
      return div;
    }),
  );

  cubes.replaceChildren(
    ...r.cubes.map((c) => {
      const tr = document.createElement("tr");
      const pos = c.pos ? `${c.pos.x.toFixed(1)}, ${c.pos.y.toFixed(1)}, ${c.pos.z.toFixed(1)}` : "—";
      tr.append(cell(c.id), cell(c.state), cell(pos), cell(time(c.lastConfirmedAt)));
      return tr;
    }),
  );

  history.replaceChildren(
    ...r.history
      .slice(-60)
      .reverse()
      .map((h) => {
        const tr = document.createElement("tr");
        tr.append(
          cell(time(h.at)),
          cell(h.event),
          cell(h.id ?? "—"),
          cell(h.provenance ?? "—"),
          cell(h.outcome, h.outcome.startsWith("rejected") ? "rejected" : undefined),
        );
        return tr;
      }),
  );
}

function renderCapture(c: CaptureStatus | null): void {
  const section = document.querySelector<HTMLElement>("#capture")!;
  section.hidden = c === null;
  if (!c) return;
  const notes = document.querySelector<HTMLElement>("#capture-notes")!;
  const lines = [...c.notes, ...(c.error ? [`Error: ${c.error}`] : [])];
  notes.hidden = lines.length === 0;
  notes.textContent = lines.join(" ");

  const s = c.streams;
  const items: [string, string][] = [
    ["Capture", c.running ? "running" : "stopped"],
    ["Library", c.library ?? "—"],
    ["Adapters", c.devices.join("; ") || "—"],
    ["Packets", String(c.packets)],
    ["Game connection", s.currentStream ?? "not found yet"],
    ["Game frames", String(c.frames)],
    ["Bundles (bad)", `${s.framing.bundles} (${s.framing.badBundles})`],
    ["Resyncs / gaps", `${s.framing.resyncs} / ${s.gaps}`],
    ["Decoder profile", c.profile ? `${c.profile}${c.profileValidated ? " (validated)" : " (NOT validated)"}` : "none"],
    ["Decoded / unreadable", `${c.decoded} / ${c.decodeErrors}`],
    ["Recording", c.recording ? `${c.recording} (${Math.round(c.recordingBytes / 1024)} KB)` : "off"],
    ["Markers", c.markersFile ? `${c.markers} in ${c.markersFile}` : "—"],
  ];
  document.querySelector<HTMLElement>("#capture-summary")!.replaceChildren(
    ...items.map(([k, v]) => {
      const div = document.createElement("div");
      const span = document.createElement("span");
      span.textContent = `${k}: `;
      div.append(span, v);
      return div;
    }),
  );
  document.querySelector<HTMLTableSectionElement>("#capture-census tbody")!.replaceChildren(
    ...c.census.map((r) => {
      const tr = document.createElement("tr");
      tr.append(
        cell(r.opcode),
        cell(String(r.count)),
        cell(r.minLen === r.maxLen ? String(r.minLen) : `${r.minLen}–${r.maxLen}`),
        cell(String(r.bundled)),
        cell(r.decoded ? "yes" : ""),
      );
      return tr;
    }),
  );
}

async function fillDevices(): Promise<void> {
  const select = form.querySelector<HTMLSelectElement>('[name="dataSource.device"]')!;
  try {
    const devices = await backend.listCaptureDevices();
    for (const d of devices) {
      const label = `${d.description ?? d.name}${d.ipv4.length ? ` (${d.ipv4.join(", ")})` : ""}`;
      select.append(new Option(label, d.name));
    }
  } catch (e) {
    select.append(new Option(`Capture library unavailable: ${String(e)}`, "", false, false));
  }
}

async function main(): Promise<void> {
  document.querySelector<HTMLElement>("#preview-note")!.hidden = !backend.isPreview;
  for (const sel of form.querySelectorAll<HTMLSelectElement>("select[data-axis]")) {
    sel.append(...AXES.map((a) => new Option(AXIS_LABEL[a], a)));
  }
  await fillDevices();
  fill(await backend.getSettings());
  await backend.onSettings(fill);

  form.querySelector('[name="dataSource.kind"]')!.addEventListener("change", (e) => {
    showSourceSection((e.target as HTMLSelectElement).value);
  });
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    try {
      const res = await backend.updateSettings(read());
      fill(res.settings);
      showWarnings(res.warnings);
    } catch (err) {
      showWarnings([`Could not apply settings: ${String(err)}`]);
    }
  });
  document.querySelector("#reset")!.addEventListener("click", async () => {
    const res = await backend.resetSettings();
    fill(res.settings);
    showWarnings(res.warnings);
  });

  for (const [id, kind] of [
    ["#mark-visible", "cube_visible"],
    ["#mark-collected", "cube_collected"],
  ] as const) {
    document.querySelector(id)!.addEventListener("click", async () => {
      try {
        await backend.addCaptureMarker(kind);
      } catch (err) {
        showWarnings([String(err)]);
      }
    });
  }

  const poll = async () => {
    try {
      renderDebug(await backend.getDebugReport());
      renderCapture(await backend.getCaptureStatus());
    } catch (err) {
      console.error(err);
    }
  };
  await poll();
  window.setInterval(poll, 1000);
}

main().catch((e) => showWarnings([`Settings failed to load: ${String(e)}`]));
