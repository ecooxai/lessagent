"use strict";
const $ = (s) => document.querySelector(s),
  el = (tag, cls, text) => {
    const n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text !== undefined) n.textContent = text;
    return n;
  };
let token =
  new URLSearchParams(location.hash.slice(1)).get("token") ||
  sessionStorage.getItem("lessagent-token") ||
  "";
if (token) sessionStorage.setItem("lessagent-token", token);
history.replaceState(null, "", location.pathname);
let state = null,
  selected = null,
  inventory = null,
  viewKey = "",
  chatStamp = "",
  terminalStamp = "",
  drafts = {},
  polling = false,
  peer = null,
  stream = null,
  channel = null,
  voiceWorkspace = null,
  blobUrls = [],
  noticeTimer;
const api = async (path, body, raw = false) => {
  const r = await fetch(path, {
    method: body === undefined ? "GET" : "POST",
    headers: {
      ...(token ? {Authorization: `Bearer ${token}`} : {}),
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (r.status === 401 && path !== "/api/login") {
    if (token) {
      token = "";
      sessionStorage.removeItem("lessagent-token");
      return api(path, body, raw);
    }
    showLogin();
  }
  if (!r.ok) {
    let message = await r.text();
    try {
      message = JSON.parse(message).error || message;
    } catch {}
    throw Error(message);
  }
  return raw ? r : r.json();
};
function showLogin() {
  document.body.classList.add("authenticating");
  const dialog = $("#login-dialog");
  if (!dialog.open) { dialog.showModal(); $("#login-password").focus(); }
}
$("#login-dialog").addEventListener("cancel", event => event.preventDefault());
$("#login-form").addEventListener("submit", async event => {
  event.preventDefault();
  const button = event.submitter;
  button.disabled = true;
  $("#login-error").textContent = "";
  try {
    const result = await api("/api/login", {password: $("#login-password").value});
    token = result.token;
    sessionStorage.setItem("lessagent-token", token);
    $("#login-password").value = "";
    $("#login-dialog").close();
    document.body.classList.remove("authenticating");
    await poll();
  } catch (error) { $("#login-error").textContent = error.message; }
  finally { button.disabled = false; }
});
const act = (name, body = {}) => api(`/api/action/${name}`, body);
const workspace = () => state?.workspaces.find((w) => w.id === selected);
const guard =
  (fn) =>
  (...args) => {
    try {
      return Promise.resolve(fn(...args)).catch(notice);
    } catch (e) {
      notice(e);
    }
  };
function notice(error) {
  $("#notice").hidden = false;
  $("#notice").textContent = error.message || String(error);
  clearTimeout(noticeTimer);
  noticeTimer = setTimeout(() => ($("#notice").hidden = true), 15000);
}
function button(text, fn, cls) {
  const b = el("button", cls, text);
  b.type = "button";
  b.onclick = guard(fn);
  return b;
}
function ask(title, label, value = "") {
  return new Promise((resolve) => {
    const d = $("#dialog");
    $("#dialog-title").textContent = title;
    $("#dialog-label").textContent = label;
    $("#dialog-value").value = value;
    d.returnValue = "cancel";
    d.onclose = () =>
      resolve(d.returnValue === "ok" ? $("#dialog-value").value : null);
    d.showModal();
    $("#dialog-value").focus();
  });
}
function ui() {
  const w = workspace();
  return w?.ui?.tabs
    ? w.ui
    : { tabs: [{ id: "chat", kind: "chat", title: "Agent" }], active: "chat" };
}
async function saveDraft(id, text) {
  drafts[id] = text;
  const w = state?.workspaces.find((w) => w.id === id);
  if (w) {
    w.ui = { ...w.ui, draft: text };
    await act("draft", { workspace: id, text });
  }
}
async function saveUi() {
  if (workspace()) await act("ui", { workspace: selected, ui: workspace().ui });
}
async function select(id) {
  selected = id;
  inventory = null;
  viewKey = "";
  await act("ui", { ui: { ...state.ui, selected: id } });
  render();
  await scan();
}
async function openTab(kind, title, extra = {}) {
  if (!workspace() && kind !== "settings" && kind !== "logs") {
    notice("Open a workspace folder first.");
    return;
  }
  if (!workspace()) {
    viewKey = "";
    showStandalone(kind);
    return;
  }
  const u = ui(),
    id = extra.id || kind;
  let t = u.tabs.find((t) => t.id === id);
  if (!t) {
    t = { id, kind, title, ...extra };
    u.tabs.push(t);
  }
  u.active = id;
  workspace().ui = u;
  viewKey = "";
  render();
  await saveUi();
}
async function closeTab(id) {
  const u = ui();
  u.tabs = u.tabs.filter((t) => t.id !== id);
  if (u.active === id) u.active = "chat";
  workspace().ui = u;
  viewKey = "";
  render();
  await saveUi();
}
async function scan() {
  if (!selected) return;
  const id = selected;
  const inv = await api(`/api/inventory/${id}`);
  if (id === selected) {
    inventory = inv;
    renderContext();
    if (ui().active === "files") renderFiles();
    if (ui().active === "context") renderContextDetails();
  }
}
function renderContext() {
  const c = $("#context");
  c.replaceChildren(el("span", "", "◫  Project context"));
  if (inventory) c.append(el("span", "context-total", formatTokens(inventory.total_tokens)));
  c.title = "View context by folder and file";
}
const formatTokens = n => (n / 1000).toLocaleString(undefined, {maximumFractionDigits: n < 100 ? 3 : 1, minimumFractionDigits: 1}) + "k tokens";
function renderContextDetails() {
  const root = $("#view");
  if (root.querySelector(".file-browser[data-context]")) { root.querySelector(".file-browser").refreshTree?.(); return; }
  root.replaceChildren();
  root.append(createFileBrowser({workspaceId:selected, root:workspace().path, context:true}));
}
function showMenu(anchor, items) {
  const menu = $("#menu"); menu.replaceChildren();
  for (const item of items) {
    const b = button(item.label, async () => { menu.hidden = true; await item.run(); });
    b.disabled = !!item.disabled; menu.append(b);
  }
  menu.hidden = false;
  const r = anchor.getBoundingClientRect();
  menu.style.left = Math.max(8, Math.min(r.left, innerWidth-menu.offsetWidth-8)) + "px";
  menu.style.top = Math.min(r.bottom+5, innerHeight-menu.offsetHeight-8) + "px";
  menu.querySelector("button:not(:disabled)")?.focus();
}
function workspaceMenu(anchor, id) {
  const w = state.workspaces.find(w => w.id === id);
  showMenu(anchor, [
    {label: (w.mode === "normal" ? "✓ " : "") + "Normal mode", run: () => setMode(id, "normal")},
    {label: (w.mode === "light" ? "✓ " : "") + "Light mode", disabled: id === selected && inventory && !inventory.light_allowed, run: () => setMode(id, "light")},
  ]);
}
async function setMode(id, mode) { await act("workspace_mode", {workspace:id, mode}); await poll(); }
function newTabMenu(anchor) {
  showMenu(anchor, [
    {label:"Files", run: async () => { await openTab("files", "Files"); await scan(); }},
    {label:"Terminal", run:newTerminal},
    {label:"Managed terminals", run: () => openTab("managed", "Managed terminals")},
    {label:"Webview", run:newWebview},
  ]);
}

function render() {
  if (!state) return;
  const w = workspace();
  document.body.dataset.mode = w?.mode || "normal";
  $("#workspace-title").textContent = w?.path || "Open a folder";
  $("#workspace-title").title = w?.path || "";
  $("#provider-label").textContent =
    `${state.settings.provider} / ${state.settings.model || "default model"}`;
  const list = $("#workspaces");
  const workspaceStamp = JSON.stringify(state.workspaces.map(x => [x.id, x.path]));
  if (list.dataset.stamp !== workspaceStamp) {
  list.dataset.stamp = workspaceStamp;
  list.replaceChildren();
  for (const x of state.workspaces) {
    const b = button(
      x.path.split("/").filter(Boolean).at(-1) || x.path,
      () => { if (selected !== x.id) return select(x.id); },
      "workspace" + (x.id === selected ? " active" : ""),
    );
    b.dataset.workspace = x.id;
    b.title = x.path;
    b.ondblclick = () => workspaceMenu(b, x.id);
    b.oncontextmenu = e => { e.preventDefault(); workspaceMenu(b, x.id); };
    list.append(b);
  }
  }
  for (const b of list.children) b.classList.toggle("active", b.dataset.workspace === selected);
  renderContext();
  const tabs = $("#tabs");
  tabs.replaceChildren();
  const add = button("＋", e => newTabMenu(e.currentTarget), "new-tab");
  add.title = "New tab"; add.setAttribute("aria-label", "New tab"); tabs.append(add);
  if (w) {
    for (const t of ui().tabs) {
      const b = button(
        t.title,
        async () => {
          ui().active = t.id;
          viewKey = "";
          render();
          await saveUi();
        },
        ui().active === t.id ? "active" : "",
      );
      if (t.id !== "chat") {
        const x = el("span", "close", "×");
        x.onclick = guard(async (e) => {
          e.stopPropagation();
          await closeTab(t.id);
        });
        b.append(x);
      }
      tabs.append(b);
    }
  }
  const t = ui().tabs.find((t) => t.id === ui().active) || {
    kind: "chat",
    id: "chat",
  };
  const key = `${selected}/${t.id}`;
  if (key !== viewKey) {
    viewKey = key;
    chatStamp = "";
    terminalStamp = "";
    for (const u of blobUrls) URL.revokeObjectURL(u);
    blobUrls = [];
    $("#view").replaceChildren();
    renderView(t);
  } else if (t.kind === "chat") updateChat();
  else if (["terminals", "managed"].includes(t.kind)) updateTerminals();
  else if (t.kind === "logs") renderLogs();
}
function showStandalone(kind) {
  $("#view").replaceChildren();
  viewKey = "standalone/" + kind;
  renderView({ kind });
}
function renderView(t) {
  ({
    chat: renderChat,
    terminals: renderTerminals,
    managed: () => renderTerminals(true),
    context: renderContextDetails,
    settings: renderSettings,
    logs: renderLogs,
    files: renderFiles,
    file: () => guard(renderFile)(t),
    web: () => renderWeb(t),
  })[t.kind]?.();
}
function renderChat() {
  const root = el("div", "chat"),
    messages = el("div", "messages");
  messages.id = "messages";
  root.append(messages);
  const form = el("form", "composer"),
    input = el("textarea");
  input.placeholder = "Give your agent a task…";
  input.setAttribute("aria-label", "Task prompt");
  input.value = drafts[selected] ?? workspace()?.ui?.draft ?? "";
  const draftWorkspace = selected;
  input.oninput = guard(() => saveDraft(draftWorkspace, input.value));
  input.onkeydown = (e) => {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      form.requestSubmit();
    }
  };
  const row = el("div", "composer-actions"),
    left = el("div", "row");
  const model = el("select", "composer-model");
  model.setAttribute("aria-label", "Model");
  const selectedModel = state.settings.model;
  const option = el("option", "", selectedModel || "Default model"); option.value = selectedModel; model.append(option);
  model.onchange = guard(async () => { await act("settings", {...state.settings, model:model.value}); await poll(); });
  api("/api/models/" + state.settings.provider).then(models => {
    if (!model.isConnected) return;
    for (const m of models) if (m.id !== selectedModel) { const o = el("option", "", m.name || m.id); o.value = m.id; model.append(o); }
  }).catch(e => { model.title = "Model discovery unavailable: " + e.message; });
  left.append(model, iconButton(peer ? "◉" : "◎", peer ? "End voice" : "Voice", toggleVoice), iconButton("♫", "Upload audio", uploadAudio));
  const right = el("div", "row");
  const stop = button("Stop agent", async () => {
    const j = state.jobs.find(
      (j) => j.workspace === selected && j.status === "running",
    );
    if (j) await act("stop", { job_id: j.id });
  });
  stop.id = "stop-job";
  const submit = el("button", "primary icon-button", "↑");
  submit.title = "Send task"; submit.setAttribute("aria-label", "Send task");
  submit.type = "submit";
  submit.id = "run-job";
  right.append(stop, submit);
  row.append(left, right);
  form.append(input, row);
  form.onsubmit = guard(async (e) => {
    e.preventDefault();
    if (!workspace()) {
      notice("Use the folder button in the sidebar to open a folder.");
      return;
    }
    await act("run", { workspace: selected, prompt: input.value });
    input.value = "";
    await saveDraft(selected, "");
    await poll();
  });
  root.append(form);
  $("#view").append(root);
  updateChat();
}
function updateChat() {
  const w = workspace(),
    m = $("#messages");
  if (!m) return;
  const jobs = state.jobs.filter((j) => j.workspace === selected);
  const current = jobs.at(-1),
    running = current?.status === "running";
  $("#run-job").disabled = running || !w;
  $("#stop-job").hidden = !running;
  const stamp = JSON.stringify([w?.messages, current]);
  if (stamp === chatStamp) return;
  chatStamp = stamp;
  const nearBottom = m.scrollHeight - m.scrollTop - m.clientHeight < 80;
  m.replaceChildren();
  if (!w?.messages.length) {
    const welcome = el("div", "welcome");
    welcome.append(

      el(
        "h2",
        "",
        w ? "What shall we work on?" : "Your workspace, always ready.",
      ),
      el(
        "p",
        "",
        w
          ? ""
          : "Open a local folder to start. Connect a model, give it a task, and follow the work in persistent terminals.",
      ),
    );
    const chips = el("div", "chips");
    for (const text of [
      "Explore this project",
      "Run the tests",
      "Find improvements",
    ])
      chips.append(
        button(text, () => {
          const input = $(".composer textarea");
          input.value = text;
          guard(saveDraft)(selected, text);
          input.focus();
        }),
      );
    welcome.append(chips);
    m.append(welcome);
  }
  for (const msg of w?.messages || []) {
    const block = el("div", "message " + msg.role);
    block.append(
      el("span", "role", msg.role === "user" ? "You" : "Lessagent"),
      document.createTextNode(msg.text),
    );
    if (msg.role === "assistant")
      block.append(button("Listen", () => speak(msg.text)));
    m.append(block);
  }
  if (current?.events.length) {
    const details = el("details");
    details.open = running;
    details.append(
      el(
        "summary",
        "muted",
        `${current.status} · ${current.events.length} events`,
      ),
    );
    const events = el(
      "div",
      "events",
      current.events
        .map((e) =>
          e.kind === "model"
            ? `→ ${e.provider} · step ${e.step}`
            : e.kind === "text"
              ? e.text
              : e.kind === "tool"
                ? `$ ${e.name} ${JSON.stringify(e.arguments)}`
                : JSON.stringify(e.result),
        )
        .join("\n\n"),
    );
    details.append(events);
    m.append(details);
  }
  if (nearBottom || running) m.scrollTop = m.scrollHeight;
}
function toolbar(title, ...actions) {
  const t = el("div", "toolbar");
  t.append(el("h2", "", title));
  const row = el("div", "row");
  row.append(...actions);
  t.append(row);
  return t;
}
function renderTerminals(managed = false) {
  $("#view").append(
    toolbar(managed ? "Managed terminals" : "Terminals", ...(managed ? [] : [button("＋", newTerminal)])),
  );
  const grid = el("div", "terminal-grid");
  grid.id = "terminal-grid";
  grid.dataset.managed = String(managed);
  $("#view").append(grid);
  updateTerminals();
}
async function newTerminal() {
  if (!workspace()) return notice("Open a workspace first.");
  await act("terminal_new", { workspace: selected });
  await openTab("terminals", "Terminals");
  await poll();
}
function updateTerminals() {
  const grid = $("#terminal-grid");
  if (!grid) return;
  const ts = state.terminals.filter(t => t.workspace === selected && !!t.managed === (grid.dataset.managed === "true"))
    .sort((a,b) => Number(b.running)-Number(a.running) || Number(a.exited)-Number(b.exited) || b.created-a.created);
  grid.querySelector(".empty")?.remove();
  if (!ts.length) grid.append(el("p", "empty", grid.dataset.managed === "true" ? "Terminals started by the agent appear here." : "No terminals yet."));
  for (const node of [...grid.children])
    if (node.dataset.id && !ts.some((t) => t.id === node.dataset.id)) node.remove();
  for (const t of ts) {
    let card = [...grid.children].find((n) => n.dataset.id === t.id);
    if (!card) {
      card = el("div", "terminal");
      card.dataset.id = t.id;
      const height = workspace()?.ui?.terminal_heights?.[t.id];
      if (height) card.style.height = `${height}px`;
      const h = el("header"),
        title = el("span", "", t.title);
      title.title = t.title;
      h.append(
        title,
        button("Split", newTerminal),
        button("Stop", () =>
          act("tool", {
            workspace: selected,
            name: "terminal_stop",
            arguments: { terminal_id: t.id },
          }),
        ),
        button("×", async () => {
          await act("terminal_remove", { terminal_id: t.id });
          await poll();
        }),
      );
      const screen = el("div", "terminal-surface");
      card.append(h, screen);
      grid.append(card);
      mountTerminal(screen, t.id, selected);
    }
    card.style.order = ts.indexOf(t);
    card.querySelector("header span").textContent =
      `${t.running ? "●" : "○"} ${t.title}${!t.exited ? (t.running ? " · running" : " · idle") : ""}${t.exited ? " · exit " + (t.exit_code ?? "archived") : ""}`;
    card.querySelector("header button:nth-of-type(2)").disabled = t.exited;
  }
}
const sendTerminal = (id, text) =>
  act("tool", {
    workspace: selected,
    name: "terminal_write",
    arguments: { terminal_id: id, text },
  });
function field(label, control, hint) {
  const l = el("label", "field");
  l.append(el("span", "", label), control);
  if (hint) l.append(el("small", "", hint));
  return l;
}
function input(value, type = "text") {
  const i = el("input");
  i.type = type;
  i.value = value;
  return i;
}
function renderSettings() {
  const root = el("div", "settings");
  root.append(
    el("h2", "", "Settings"),
    el("p", "muted", "Connections and preferences are stored by the backend."),
  );
  const provider = el("select");
  for (const p of ["codex", "openai", "gemini", "claude"]) {
    const o = el("option", "", p === "codex" ? "Codex login" : p);
    o.value = p;
    provider.append(o);
  }
  provider.value = state.settings.provider;
  const model = input(state.settings.model),
    list = el("datalist");
  list.id = "models";
  model.setAttribute("list", "models");
  const refresh = button("Discover models", async () => {
    refresh.disabled = true;
    try {
      const models = await api("/api/models/" + provider.value);
      list.replaceChildren();
      for (const m of models) {
        const o = el("option");
        o.value = m.id;
        o.label = m.name;
        list.append(o);
      }
      model.placeholder = `${models.length} models available`;
      notice(`${models.length} models loaded. Choose one in the Model field.`);
    } finally {
      refresh.disabled = false;
    }
  });
  provider.onchange = () => {
    model.value = "";
    list.replaceChildren();
  };
  const steps = input(state.settings.max_steps, "number");
  steps.min = 1;
  steps.max = 100;
  const computer = input("", "checkbox");
  computer.checked = state.settings.computer_enabled;
  const ignores = el("textarea", "ignore-editor");
  ignores.value = (state.settings.context_ignores || []).join("\n");
  ignores.setAttribute("aria-label", "Context ignore patterns");
  root.append(
    field(
      "Provider",
      provider,
      "Codex uses your existing CLI login. Run codex login in a terminal first.",
    ),
    field(
      "Model",
      model,
      "Discover available models or enter a model id. Blank uses the default for Codex only.",
    ),
    list,
    refresh,
    field("Context ignore patterns", ignores, "One gitignore-style pattern per line. Applies to AI context only; Files still shows excluded files. Project .gitignore rules also apply."),
    button("Reset ignore defaults", () => { ignores.value = state.context_ignore_defaults.join("\n"); }),
    field("Maximum model calls per task", steps),
    field(
      "Enable mouse, keyboard, and screenshots",
      computer,
      JSON.stringify(state.computer),
    ),
    button(
      "Save settings",
      async () => {
        await act("settings", {
          provider: provider.value,
          model: model.value,
          max_steps: Number(steps.value),
          computer_enabled: computer.checked,
          context_ignores: ignores.value.split("\n").map(s => s.trim()).filter(Boolean),
        });
        await poll();
        await scan();
        notice("Settings saved.");
      },
      "primary",
    ),
    el("hr"),
  );
  root.append(el("h3", "", "API connections"));
  for (const p of ["openai", "claude", "gemini"]) {
    const key = input("", "password");
    key.autocomplete = "new-password";
    key.placeholder = state.keys[p]
      ? "Connected · enter a replacement key"
      : "API key";
    const row = el("div", "row");
    row.append(
      key,
      button("Save", async () => {
        await act("key", { provider: p, key: key.value });
        key.value = "";
        await poll();
        notice(`${p} key saved.`);
      }),
    );
    root.append(
      field(
        p,
        row,
        "Saved keys are local files with owner-only permissions. Environment variables take precedence.",
      ),
    );
  }
  root.append(el("hr"), el("h3", "", "OpenAI audio"));
  for (const [name, label, fallback] of [
    ["realtimeModel", "Realtime model", "gpt-realtime"],
    ["speechModel", "Speech model", "gpt-4o-mini-tts"],
    ["transcribeModel", "Transcription model", "gpt-4o-mini-transcribe"],
  ]) {
    const i = input(state.ui?.[name] || fallback);
    i.onchange = guard(async () => {
      state.ui = { ...state.ui, [name]: i.value };
      await act("ui", { ui: state.ui });
    });
    root.append(field(label, i));
  }
  root.append(
    el(
      "p",
      "muted",
      "Voice uses your microphone and OpenAI API key. AI-generated speech plays in the browser. Realtime tool calls use the workspace selected when the call starts.",
    ),
  );
  $("#view").append(root);
}
function renderLogs() {
  const v = $("#view");
  let logs = v.querySelector(".logs");
  if (!logs) {
    v.replaceChildren(toolbar("Backend logs", button("Refresh", poll)));
    logs = el("pre", "logs");
    v.append(logs);
  }
  logs.textContent =
    state.logs
      .map(
        (l) =>
          `${new Date(l.at).toLocaleTimeString()}  ${l.kind.padEnd(8)} ${l.message}`,
      )
      .join("\n") || "No backend events yet.";
}
function renderFiles() {
  if ($("#view .file-browser")) return;
  $("#view").replaceChildren(createFileBrowser({workspaceId:selected, root:workspace().path}));
}
async function renderFile(t) {
  const w = selected,
    key = viewKey;
  const v = $("#view");
  v.append(toolbar(t.path));
  const r = await api("/api/media/" + w, { path: t.path }, true);
  if (key !== viewKey) return;
  const mime = r.headers.get("content-type") || "";
  if (
    mime.startsWith("image/") ||
    mime.startsWith("audio/") ||
    mime.startsWith("video/")
  ) {
    const url = URL.createObjectURL(await r.blob());
    blobUrls.push(url);
    const tag = mime.startsWith("image/")
      ? "img"
      : mime.startsWith("audio/")
        ? "audio"
        : "video";
    const node = el(tag, "preview");
    node.src = url;
    if (tag !== "img") node.controls = true;
    else node.alt = t.path;
    v.append(node);
  } else if (t.kindOfFile === "binary") {
    v.append(el("p", "muted", "Binary file. Preview is not available."));
  } else {
    const editor = el("textarea", "editor");
    editor.setAttribute("aria-label", t.path);
    editor.spellcheck = false;
    editor.value = await r.text();
    v.firstChild.append(
      button(
        "Save file",
        async () => {
          await act("tool", {
            workspace: w,
            name: "write_file",
            arguments: { path: t.path, text: editor.value },
          });
          notice("File saved.");
          await scan();
        },
        "primary",
      ),
    );
    v.append(editor);
  }
}
function renderWeb(t) {
  const v = $("#view");
  const link = el("a", "", "Open in browser ↗");
  link.href = t.url;
  link.target = "_blank";
  link.rel = "noopener noreferrer";
  v.append(toolbar(t.url, link));
  const frame = el("iframe", "webframe");
  frame.src = t.url;
  frame.title = t.url;
  frame.setAttribute("sandbox", "allow-scripts allow-forms allow-popups");
  frame.referrerPolicy = "no-referrer";
  v.append(
    el(
      "p",
      "muted",
      "Some websites block embedding. Use “Open in browser” if the page is unavailable.",
    ),
    frame,
  );
}
async function uploadAudio() {
  const picker = el("input");
  picker.type = "file";
  picker.accept = "audio/*";
  picker.onchange = guard(async () => {
    const file = picker.files[0];
    if (!file) return;
    notice("Transcribing audio…");
    const data = await new Promise((resolve, reject) => {
      const r = new FileReader();
      r.onload = () => resolve(r.result.split(",")[1]);
      r.onerror = reject;
      r.readAsDataURL(file);
    });
    const result = await api("/api/audio/transcribe", {
      filename: file.name,
      data,
      model: state.ui?.transcribeModel || "gpt-4o-mini-transcribe",
    });
    const text = $(".composer textarea");
    if (text) {
      text.value += (text.value ? "\n" : "") + result.text;
      await saveDraft(selected, text.value);
    }
    notice("Transcription ready.");
  });
  picker.click();
}
async function speak(text) {
  notice("Generating AI speech…");
  const r = await api(
    "/api/audio/speech",
    { text, model: state.ui?.speechModel || "gpt-4o-mini-tts" },
    true,
  );
  const url = URL.createObjectURL(await r.blob());
  blobUrls.push(url);
  const a = el("audio", "audio-result");
  a.src = url;
  a.controls = true;
  $("#view").append(a);
  await a.play();
}
function endVoice() {
  peer?.close();
  stream?.getTracks().forEach((t) => t.stop());
  peer = null;
  stream = null;
  channel = null;
  $("#voice-audio")?.remove();
  voiceWorkspace = null;
  notice("Voice session ended.");
}
async function toggleVoice() {
  if (peer) {
    endVoice();
    return;
  }
  if (!selected) return notice("Open a workspace first.");
  voiceWorkspace = selected;
  try {
    stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    peer = new RTCPeerConnection();
    const audio = el("audio");
    audio.id = "voice-audio";
    audio.autoplay = true;
    document.body.append(audio);
    peer.ontrack = (e) => (audio.srcObject = e.streams[0]);
    for (const track of stream.getTracks()) peer.addTrack(track, stream);
    channel = peer.createDataChannel("oai-events");
    channel.onmessage = guard(async (event) => {
      const m = JSON.parse(event.data);
      if (m.type === "error") notice(m.error?.message || "Realtime error");
      if (m.type === "response.function_call_arguments.done") {
        let result;
        try {
          result = await act("tool", {
            workspace: voiceWorkspace,
            name: m.name,
            arguments: JSON.parse(m.arguments),
          });
        } catch (e) {
          result = { error: e.message };
        }
        if (result.image) {
          channel.send(
            JSON.stringify({
              type: "conversation.item.create",
              item: {
                type: "message",
                role: "user",
                content: [
                  {
                    type: "input_image",
                    image_url: `data:${result.image.mime};base64,${result.image.data}`,
                  },
                ],
              },
            }),
          );
          delete result.image;
        }
        channel.send(
          JSON.stringify({
            type: "conversation.item.create",
            item: {
              type: "function_call_output",
              call_id: m.call_id,
              output: JSON.stringify(result),
            },
          }),
        );
        channel.send(JSON.stringify({ type: "response.create" }));
      }
      if (m.type === "response.output_audio_transcript.done")
        notice(m.transcript);
    });
    peer.onconnectionstatechange = () => {
      if (peer?.connectionState === "failed") endVoice();
    };
    const offer = await peer.createOffer();
    await peer.setLocalDescription(offer);
    const r = await api(
      "/api/audio/realtime",
      { sdp: offer.sdp, model: state.ui?.realtimeModel || "gpt-realtime" },
      true,
    );
    await peer.setRemoteDescription({ type: "answer", sdp: await r.text() });
    notice("Voice connected. Microphone is active. Click Voice again to end.");
  } catch (e) {
    endVoice();
    throw e;
  }
}
async function poll() {
  if (polling || $("#login-dialog").open) return;
  polling = true;
  try {
    state = await api("/api/state");
    if (!selected || !state.workspaces.some((w) => w.id === selected))
      selected = state.ui?.selected || state.workspaces[0]?.id || null;
    $("#connection").textContent = "●";
    $("#connection").title = "Backend connected";
    if (!viewKey.startsWith("standalone/")) render();
  } catch (e) {
    $("#connection").textContent = "○";
    $("#connection").title = "Backend disconnected";
    if (!state) notice(e);
  } finally {
    polling = false;
  }
}
$("#add-workspace").onclick = guard(async () => {
  const home = await act("browse", {});
  const dialog = el("dialog", "folder-picker");
  dialog.append(createFileBrowser({root:home.path, picker:true, onChoose:async path => {
    const w = await act("workspace_open", {path});
    dialog.close(); await poll(); await select(w.id);
  }}));
  dialog.append(button("Cancel", () => dialog.close()));
  dialog.onclose = () => dialog.remove();
  document.body.append(dialog); dialog.showModal();
});
$("#settings-nav").onclick = guard(() => openTab("settings", "Settings"));
$("#logs-nav").onclick = guard(() => openTab("logs", "Logs"));
async function newWebview() {
  const value = await ask("New webview", "Website URL", "https://");
  if (!value) return;
  const url = new URL(value);
  if (!["http:", "https:"].includes(url.protocol))
    throw Error("Use an HTTP or HTTPS URL");
  await openTab("web", url.hostname, {
    id: crypto.randomUUID(),
    url: url.href,
  });
}
$("#context").onclick = guard(async () => { await openTab("context", "Context"); await scan(); });
document.addEventListener("pointerdown", e => { if (!$("#menu").contains(e.target)) $("#menu").hidden = true; });
document.addEventListener("keydown", e => { if (e.key === "Escape") $("#menu").hidden = true; });
window.addEventListener("beforeunload", () => {
  peer?.close();
  stream?.getTracks().forEach((t) => t.stop());
});
(async () => {
  await poll();
  if (selected) await guard(scan)();
  setInterval(poll, 1000);
  setInterval(() => {
    if (selected) scan().catch(() => {});
  }, 15000);
})();

function iconButton(symbol, label, run) {
  const b = button(symbol, run, "icon-button"); b.title = label; b.setAttribute("aria-label", label); return b;
}
function mountTerminal(host, id, workspaceId) {
  const viewport=el("div","vt-viewport"), screen=el("pre","vt-screen"), cursor=el("i","vt-cursor"), input=el("textarea","vt-input");
  input.setAttribute("aria-label","Terminal input"); input.autocapitalize="off"; input.autocomplete="off"; input.spellcheck=false;
  viewport.append(screen,cursor); host.append(viewport,input);
  const keys=el("div","vt-keys"); host.append(keys);
  let revision, modes={}, disposed=false, busy=false, writes=Promise.resolve(), scrollback=0, ctrl=false, copyTimer, resizeTimer;
  let viewGeneration=0;
  const scroll=delta=>{scrollback=Math.max(0,Math.min(1000,scrollback+delta));revision=undefined;viewGeneration++;void tick();};
  const send=text=>{viewGeneration++;scrollback=0; revision=undefined; writes=writes.then(()=>act("tool",{workspace:workspaceId,name:"terminal_write",arguments:{terminal_id:id,text}})).catch(notice);};
  for(const [label,value] of [["Esc","\x1b"],["Tab","\t"],["Ctrl",null],["↑","\x1b[A"],["↓","\x1b[B"],["←","\x1b[D"],["→","\x1b[C"]]) keys.append(iconButton(label,label,()=>{if(value)send(value);else {ctrl=!ctrl;keys.classList.toggle("ctrl-active",ctrl);} input.focus();}));
  keys.append(iconButton("Copy","Copy terminal selection",()=>navigator.clipboard.writeText(getSelection()?.toString()||screen.innerText).catch(notice)), iconButton("⇞","Scroll terminal up",()=>{scroll(10);}),iconButton("⇟","Scroll terminal down",()=>{scroll(-10);}));
  const palette=["#17221d","#df6c75","#99c794","#fac863","#6699cc","#c594c5","#5fb3b3","#d8dee9","#65737e","#ec5f67","#99c794","#fac863","#6699cc","#c594c5","#5fb3b3","#ffffff"];
  function color(value,fallback){
    if(value==="Default")return fallback;
    const rgb=value.match(/Rgb\((\d+), (\d+), (\d+)\)/); if(rgb)return `rgb(${rgb[1]},${rgb[2]},${rgb[3]})`;
    const n=Number(value.match(/Idx\((\d+)\)/)?.[1]); if(n<16)return palette[n];
    if(n>=232)return `rgb(${Array(3).fill(8+(n-232)*10).join(",")})`;
    const k=n-16, level=x=>x?55+x*40:0;return `rgb(${level(Math.floor(k/36))},${level(Math.floor(k/6)%6)},${level(k%6)})`;
  }
  function render(data){
    modes=data; input.disabled=data.exited;
    const selection=getSelection(); if(selection?.toString() && screen.contains(selection.anchorNode))return;
    const rows=data.rows.map(cells=>{const row=el("div","vt-row");let span,last;
      for(const [text,fg,bg,bold,italic,underline,inverse] of cells){const key=JSON.stringify([fg,bg,bold,italic,underline,inverse]);if(key!==last){span=el("span");span.style.color=color(inverse?bg:fg,inverse?"#17221d":"#e0ebe5");span.style.backgroundColor=color(inverse?fg:bg,inverse?"#e0ebe5":"transparent");span.style.fontWeight=bold?"bold":"normal";span.style.fontStyle=italic?"italic":"normal";span.style.textDecoration=underline?"underline":"none";row.append(span);last=key;}span.textContent+=text;}return row;});
    rows.forEach((row,i)=>{if(screen.children[i]?.innerHTML!==row.innerHTML)screen.children[i]?screen.children[i].replaceWith(row):screen.append(row);});while(screen.children.length>rows.length)screen.lastChild.remove();
    cursor.style.left=`${data.cursor[1]}ch`;cursor.style.top=`${data.cursor[0]*19}px`;cursor.hidden=data.hide_cursor||data.exited||data.scrollback>0; revision=data.revision;
  }
  async function tick(){if(!host.isConnected){cleanup();return;}if(busy||document.hidden)return;busy=true;const generation=viewGeneration;try{const data=await act("terminal_screen",{terminal_id:id,revision,scrollback});if(!disposed&&generation===viewGeneration){input.disabled=data.exited;if(!data.unchanged){scrollback=data.scrollback;render(data);}}}catch(e){notice(e);cleanup();}finally{busy=false;}}
  input.onkeydown=e=>{
    if(e.isComposing||e.metaKey||(e.ctrlKey&&e.shiftKey))return;
    const map={Enter:"\r",Backspace:"\x7f",Tab:e.shiftKey?"\x1b[Z":"\t",Escape:"\x1b",ArrowUp:"A",ArrowDown:"B",ArrowRight:"C",ArrowLeft:"D",Home:"\x1b[H",End:"\x1b[F",Delete:"\x1b[3~",Insert:"\x1b[2~",PageUp:"\x1b[5~",PageDown:"\x1b[6~",F1:"\x1bOP",F2:"\x1bOQ",F3:"\x1bOR",F4:"\x1bOS",F5:"\x1b[15~",F6:"\x1b[17~",F7:"\x1b[18~",F8:"\x1b[19~",F9:"\x1b[20~",F10:"\x1b[21~",F11:"\x1b[23~",F12:"\x1b[24~"};
    let text=map[e.key];if(e.key.startsWith("Arrow"))text="\x1b"+(modes.application_cursor?"O":"[")+text;
    if((e.ctrlKey||ctrl)&&e.key.length===1)text=String.fromCharCode(e.key.toUpperCase().charCodeAt(0)&31);
    else if(e.altKey&&e.key.length===1)text="\x1b"+e.key;
    if(text){e.preventDefault();send(text);ctrl=false;keys.classList.remove("ctrl-active");}
  };
  input.oninput=e=>{if(e.isComposing)return;if(input.value)send(ctrl?String.fromCharCode(input.value.toUpperCase().charCodeAt(0)&31):input.value);else if(e.inputType==="deleteContentBackward")send("\x7f");input.value="";ctrl=false;};
  input.oncompositionend=()=>{if(input.value)send(input.value);input.value="";};
  input.onpaste=e=>{e.preventDefault();let text=e.clipboardData.getData("text");send(modes.bracketed_paste?"\x1b[200~"+text+"\x1b[201~":text);};
  viewport.onclick=()=>{if(!getSelection()?.toString())input.focus({preventScroll:true});};
  viewport.addEventListener("wheel",e=>{if(!e.deltaY)return;e.preventDefault();scroll(e.deltaY<0?3:-3);},{passive:false});
  function selectionChanged(){clearTimeout(copyTimer);const sel=getSelection(),text=sel?.toString();if(text?.length>4&&screen.contains(sel.anchorNode)&&screen.contains(sel.focusNode))copyTimer=setTimeout(async()=>{if(getSelection()?.toString()!==text)return;try{await navigator.clipboard.writeText(text);host.dataset.copy="Copied";}catch{host.dataset.copy="Use Copy to copy selection";}},3000);}
  document.addEventListener("selectionchange",selectionChanged);
  const resize=()=>{const cols=Math.max(10,Math.min(500,Math.floor(viewport.clientWidth/7.83))),rows=Math.max(2,Math.min(200,Math.floor(viewport.clientHeight/19)));revision=undefined;act("terminal_resize",{terminal_id:id,rows,cols,height:Math.round(host.closest(".terminal")?.getBoundingClientRect().height||300)}).catch(notice);};
  const observer=new ResizeObserver(()=>{clearTimeout(resizeTimer);resizeTimer=setTimeout(resize,100);});observer.observe(viewport);
  function cleanup(){if(disposed)return;disposed=true;clearInterval(timer);clearTimeout(copyTimer);clearTimeout(resizeTimer);observer.disconnect();document.removeEventListener("selectionchange",selectionChanged);}
  const timer=setInterval(tick,200);requestAnimationFrame(tick);return cleanup;
}

function createFileBrowser(options) {
  const {workspaceId, picker=false, context=false, onChoose} = options;
  const root = el("div", "file-browser"); if (context) root.dataset.context = "true";
  let cwd = options.root, inv = null, busy = false, disposed = false, terminalId = null, terminalVisible=false, dockCleanup=null;
  const expanded = new Set(), cache = new Map(), seen = new Map(), fresh = new Map();
  const pathLabel = el(picker ? "input" : "span", "browser-path");
  const suggestions = el("div", "path-suggestions"); suggestions.hidden=true;
  let suggestGeneration=0, suggestTimer, showHidden=false;
  if (picker) {
    pathLabel.type="text"; pathLabel.value=cwd; pathLabel.placeholder="Type a folder path…";
    pathLabel.setAttribute("aria-label", "Folder path"); pathLabel.autocomplete="off";
    const choosePath = async path => {
      ++suggestGeneration; clearTimeout(suggestTimer); suggestions.hidden=true;
      await navigate(path.startsWith("/") || path.startsWith("~") ? path : cwd.replace(/\/$/,"")+"/"+path); pathLabel.value=cwd;
    };
    pathLabel.oninput=() => {
      const generation=++suggestGeneration; clearTimeout(suggestTimer);
      suggestTimer=setTimeout(async () => {
        const value=pathLabel.value.trim();
        const slash=value.lastIndexOf("/");
        const parent=slash<0 ? cwd : value.slice(0,slash)||"/";
        const query=(slash<0?value:value.slice(slash+1)).toLowerCase();
        try {
          const data=await act("browse",{path:parent});
          if (generation!==suggestGeneration || !root.isConnected) return;
          const score=name => {
            name=name.toLowerCase(); let at=0, total=0;
            for(const c of query){const i=name.indexOf(c,at);if(i<0)return Infinity;total+=i-at;at=i+1;}
            return total+(name.startsWith(query)?0:100);
          };
          const matches=data.entries.filter(f=>f.directory && (showHidden || query.startsWith(".") || !f.name.startsWith(".")))
            .map(f=>({...f,score:score(f.name)})).filter(f=>Number.isFinite(f.score))
            .sort((a,b)=>a.score-b.score || a.name.localeCompare(b.name)).slice(0,30);
          suggestions.replaceChildren(...matches.map(f=>button(f.path,()=>choosePath(f.path))));
          if(!matches.length) suggestions.append(el("p","muted","No matching folders"));
          suggestions.hidden=false;
        } catch(e) {if(generation===suggestGeneration){suggestions.replaceChildren(el("p","muted",e.message));suggestions.hidden=false;}}
      },150);
    };
    pathLabel.onkeydown=guard(async e=>{
      if(e.key==="Enter"){e.preventDefault();await choosePath(pathLabel.value.trim());}
      if(e.key==="Escape"){e.preventDefault();e.stopPropagation();++suggestGeneration;clearTimeout(suggestTimer);suggestions.hidden=true;}
      if(e.key==="ArrowDown"){e.preventDefault();suggestions.querySelector("button")?.focus();}
    });
  }
  const rows = el("div","tree-rows");
  const summary = el("p","muted context-summary");
  const dock = el("div","file-terminal-dock"); dock.hidden=true;
  const selectedPath = () => cwd.slice(options.root.length).replace(/^\//,"") || ".";
  const bar = el("div","browser-toolbar");
  bar.append(iconButton("↑","Parent folder", () => navigate(cwd.slice(0,cwd.lastIndexOf("/")) || "/")), pathLabel,
    iconButton("↻","Refresh files", () => refresh(true)));
  if (!picker) bar.append(iconButton(">_","Toggle folder terminal", async () => {
    terminalVisible=!terminalVisible; dock.hidden=!terminalVisible;
    if (terminalVisible) await syncTerminal();
  }));
  if (picker) bar.append(button("Open this folder", () => onChoose(cwd), "primary"));
  if (picker) {
    const toggle=button("Show hidden folders",()=>{showHidden=!showHidden;toggle.textContent=showHidden?"Hide hidden folders":"Show hidden folders";draw();});
    bar.append(toggle);
  }
  root.append(bar,suggestions,summary,rows,dock);
  function relative(path) { return path.slice(options.root.length).replace(/^\//,""); }
  function stats(path, directory) {
    if (!inv) return null;
    const rel = relative(path);
    const files = inv.files.filter(f => directory ? (!rel || f.path.startsWith(rel+"/")) : f.path === rel);
    return files.length ? {bytes:files.reduce((n,f)=>n+f.bytes,0), tokens:files.reduce((n,f)=>n+f.tokens,0)} : null;
  }
  async function entries(path) {
    if (context) {
      const prefix = relative(path), prefixSlash = prefix ? prefix+"/" : "", result = new Map();
      for (const f of inv?.files || []) {
        if (!f.path.startsWith(prefixSlash)) continue;
        const tail=f.path.slice(prefixSlash.length), name=tail.split("/")[0], directory=tail.includes("/");
        if (!result.has(name)) result.set(name,{name,path:path.replace(/\/$/,"")+"/"+name,directory,bytes:f.bytes,kind:f.kind});
      }
      return [...result.values()];
    }
    return (await act("browse", {path,...(workspaceId?{workspace:workspaceId}:{})})).entries;
  }
  async function load(path) {
    try {
      const list=await entries(path), previous=seen.get(path), now=Date.now();
      if (previous) for (const f of list) if (!previous.has(f.path)) fresh.set(f.path,now);
      seen.set(path,new Set(list.map(f=>f.path))); cache.set(path,list);
    } catch(e) { cache.set(path,[]); notice(e); }
  }
  async function navigate(path) {
    if (!picker && path !== options.root && !path.startsWith(options.root+"/")) return;
    if (!context) path=(await act("browse",{path,...(workspaceId?{workspace:workspaceId}:{})})).path;
    cwd=path; expanded.clear(); await load(cwd); if(!disposed) draw();
    if (terminalVisible) await syncTerminal();
  }
  function draw() {
    if(picker) {if(document.activeElement!==pathLabel) pathLabel.value=cwd;} else pathLabel.textContent=cwd; pathLabel.title=cwd;
    summary.hidden=!context;
    if (context && inv) summary.textContent = `${formatTokens(inv.total_tokens)} · text ${formatTokens(inv.text_tokens)} · images ${formatTokens(inv.image_tokens)} · largest first`;
    const fragment=document.createDocumentFragment();
    function branch(path,depth) {
      const list=(cache.get(path)||[]).filter(f=>!picker || (f.directory && (showHidden || !f.name.startsWith("."))));
      list.sort((a,b) => context ? (stats(b.path,b.directory)?.tokens||0)-(stats(a.path,a.directory)?.tokens||0) || a.name.localeCompare(b.name) :
        Number(fresh.has(b.path))-Number(fresh.has(a.path)) || Number(b.directory)-Number(a.directory) || Number(a.name.startsWith("."))-Number(b.name.startsWith(".")) || a.name.localeCompare(b.name));
      for (const f of list) {
        const row=el("div","tree-row"+(fresh.has(f.path)?" new-file":"")); row.style.paddingLeft=depth*18+"px"; row.dataset.path=f.path;
        const open=expanded.has(f.path), label=el("span","tree-name",`${f.directory?(open?"▾":"▸"):f.kind==="image"?"▧":"·"}  ${f.name}`);
        row.title=f.path; row.setAttribute("role","treeitem"); row.tabIndex=0;
        if (f.directory) row.setAttribute("aria-expanded",String(open));
        const st=stats(f.path,f.directory);
        const metrics=el("span","tree-metrics",st ? `${(st.bytes/1024).toFixed(1)} KB · ${formatTokens(st.tokens)}` : f.directory ? "—" : `${(f.bytes/1024).toFixed(1)} KB · 0.0k tokens`);
        row.append(label,metrics);
        let clickTimer;
        const expand = async () => { if (expanded.has(f.path)) expanded.delete(f.path); else { expanded.add(f.path); await load(f.path); } draw(); };
        const openFile = async () => {
          if (picker) return;
          await openTab("file",f.name,{id:"file:"+relative(f.path),path:relative(f.path),kindOfFile:f.kind || "text"});
        };
        row.onclick=guard(async () => {
          if (f.directory) { clearTimeout(clickTimer); clickTimer=setTimeout(() => expand().catch(notice),230); }
          else await openFile();
        });
        row.ondblclick=guard(async () => { clearTimeout(clickTimer); if (f.directory) await navigate(f.path); });
        row.onkeydown=guard(async e => { if (e.key==="Enter") {e.preventDefault(); if (f.directory) await navigate(f.path); else await openFile();} else if (f.directory && ["ArrowRight","ArrowLeft"," "].includes(e.key)) {e.preventDefault(); await expand();} });
        fragment.append(row); if(open && f.directory) branch(f.path,depth+1);
      }
    }
    branch(cwd,0);
    if (!fragment.childNodes.length) fragment.append(el("p","muted","This folder is empty."));
    // Preserve focused row across automatic refreshes.
    const focus=rows.contains(document.activeElement)?document.activeElement.dataset.path:null;
    rows.replaceChildren(fragment); rows.setAttribute("role","tree");
    if(focus) [...rows.children].find(n=>n.dataset.path===focus)?.focus({preventScroll:true});
  }
  async function refresh(rescan=false) {
    if(busy) return; busy=true;
    try {
      if(workspaceId) inv=await api("/api/inventory/"+workspaceId);
      for(const [p,time] of fresh) if(Date.now()-time>30000) fresh.delete(p);
      await load(cwd);
      for(const path of expanded) await load(path);
      if(!disposed) draw();
    } finally {busy=false;}
  }
  async function syncTerminal(force=false) {
    const t=await act(force?"terminal_new":"terminal_cd",{workspace:workspaceId,path:selectedPath(),terminal_id:terminalId});
    terminalId=t.id; await poll(); renderDock();
  }
  function renderDock() {
    dockCleanup?.(); dock.replaceChildren();
    const choose=el("select"); choose.setAttribute("aria-label","Folder terminal");
    const sessions=state.terminals.filter(t=>t.workspace===workspaceId && (t.cwd===cwd || t.id===terminalId));
    for(const t of sessions){const o=el("option","",`${t.running?"●":"○"} ${t.title} · ${t.id.slice(0,6)}${t.exited?" · exited":""}`);o.value=t.id;choose.append(o);}
    choose.value=terminalId; choose.onchange=()=>{terminalId=choose.value;renderDock();};
    const head=el("div","dock-toolbar"); head.append(choose,iconButton("＋","New folder terminal",()=>syncTerminal(true)),iconButton("×","Hide terminal",()=>{terminalVisible=false;dock.hidden=true;dockCleanup?.();}));
    const surface=el("div","terminal-surface"); dock.append(head,surface); dockCleanup=mountTerminal(surface,terminalId,workspaceId);
  }
  root.refreshTree=()=>refresh(true).catch(notice);
  const timer=setInterval(()=>{if(!root.isConnected){disposed=true;clearInterval(timer);dockCleanup?.();return;} refresh().catch(notice);},3000);
  refresh().catch(notice);
  return root;
}
