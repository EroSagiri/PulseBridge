(function () {
  "use strict";

  const root = document.getElementById("embed");
  const identity = document.getElementById("identity");
  const heart = document.getElementById("heart");
  const bpm = document.getElementById("bpm");
  const reading = document.querySelector(".reading");
  const status = document.getElementById("status");
  const details = document.getElementById("details");
  const message = document.getElementById("message");
  const params = new URLSearchParams(window.location.search);
  const pathParts = window.location.pathname.split("/").filter(Boolean);
  const target = Number(pathParts[1]);
  const layout = pathParts[2] || "minimal";
  const state = {
    device: null,
    connected: false,
    retryMs: 1000,
    lastUpdateAt: 0,
  };

  function flag(name, fallback) {
    const value = params.get(name);
    if (value == null) return fallback;
    return !["0", "false", "off", "no"].includes(value.toLowerCase());
  }

  function applyOptions() {
    const theme = params.get("theme");
    if (theme === "light" || (theme !== "dark" && theme !== "light" &&
        window.matchMedia && window.matchMedia("(prefers-color-scheme: light)").matches)) {
      document.body.classList.add("light");
    }
    if (flag("transparent", layout === "live")) {
      document.body.classList.add("transparent");
    }
    root.dataset.layout = layout;
    identity.hidden = !flag("show_name", layout === "compact" || layout === "card");
    root.classList.toggle("show-name", !identity.hidden);
    status.hidden = !flag("show_status", layout !== "minimal");
  }

  function setStatus(name) {
    status.textContent = name.toUpperCase();
    status.className = "status " + name;
  }

  function currentView() {
    if (!state.connected || !state.device) {
      return { name: "offline", bpm: null, ageMs: null };
    }
    const d = state.device;
    const ageMs = d.age_ms + Math.max(0, Date.now() - state.lastUpdateAt);
    const online = d.presence === "online" && d.heart_rate != null;
    return {
      name: online ? "online" : d.presence,
      bpm: online ? d.heart_rate : null,
      ageMs,
    };
  }

  function render() {
    const view = currentView();
    const animate = flag("animate", layout === "live" || layout === "compact" || layout === "card");
    const hasBpm = view.bpm != null && view.name === "online";

    bpm.textContent = hasBpm ? String(view.bpm) : "--";
    reading.className = "reading " + view.name;
    setStatus(view.name);
    heart.classList.toggle("beat", hasBpm && animate);
    if (hasBpm) {
      heart.style.setProperty("--period", (60 / view.bpm).toFixed(2) + "s");
    } else {
      heart.style.removeProperty("--period");
    }

    identity.textContent = identity.hidden ? "" : "Device " + target;
    details.textContent = view.ageMs == null
      ? "waiting for device"
      : "updated " + (view.ageMs / 1000).toFixed(1) + "s ago";
    message.textContent = hasBpm
      ? "Heart rate " + view.bpm + " beats per minute"
      : "PulseBridge status " + view.name;
  }

  function handleMessage(raw) {
    let msg;
    try {
      msg = JSON.parse(raw);
    } catch (_) {
      return;
    }

    if (msg.type === "snapshot" && Array.isArray(msg.devices)) {
      state.device = msg.devices.find(function (d) {
        return d && d.device_id === target;
      }) || null;
      state.lastUpdateAt = Date.now();
      render();
      return;
    }

    if (msg.type === "metric" && msg.event &&
        msg.event.metric === "heart_rate" && msg.event.device_id === target) {
      if (!state.device) {
        state.device = {
          device_id: target,
          presence: "online",
          age_ms: 0,
          heart_rate: msg.event.bpm,
          contact_ok: msg.event.contact_ok,
        };
      } else {
        state.device.heart_rate = msg.event.bpm;
        state.device.contact_ok = msg.event.contact_ok;
        state.device.presence = "online";
        state.device.age_ms = 0;
      }
      state.lastUpdateAt = Date.now();
      render();
    }
  }

  function connect() {
    const proto = window.location.protocol === "https:" ? "wss" : "ws";
    const socket = new WebSocket(proto + "://" + window.location.host + "/ws");

    socket.onopen = function () {
      state.connected = true;
      state.retryMs = 1000;
      render();
    };

    socket.onmessage = function (event) {
      handleMessage(event.data);
    };

    socket.onclose = function () {
      state.connected = false;
      state.device = null;
      state.lastUpdateAt = 0;
      render();
      window.setTimeout(connect, state.retryMs);
      state.retryMs = Math.min(state.retryMs * 2, 30000);
    };

    socket.onerror = function () {
      socket.close();
    };
  }

  applyOptions();
  render();
  window.setInterval(render, 1000);
  connect();
})();
