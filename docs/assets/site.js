(function () {
  "use strict";

  var doc = document;
  var mq = window.matchMedia ? window.matchMedia("(prefers-reduced-motion: reduce)") : { matches: false };
  var reduced = mq.matches;

  function $(s, r) {
    return (r || doc).querySelector(s);
  }

  function $$(s, r) {
    return Array.prototype.slice.call((r || doc).querySelectorAll(s));
  }

  function pad(n) {
    return n < 10 ? "0" + n : String(n);
  }

  function clock(withSeconds) {
    var d = new Date();
    return pad(d.getHours()) + ":" + pad(d.getMinutes()) + (withSeconds ? ":" + pad(d.getSeconds()) : "");
  }

  function span(cls, text) {
    var s = doc.createElement("span");
    if (cls) s.className = cls;
    if (text != null) s.textContent = text;
    return s;
  }

  function fill(el, kids) {
    kids.forEach(function (k) {
      el.appendChild(typeof k === "string" ? doc.createTextNode(k) : k);
    });
    return el;
  }

  function detectOS() {
    var uad = navigator.userAgentData;
    var p = (uad && uad.platform) || navigator.platform || "";
    var ua = navigator.userAgent || "";
    if (/win/i.test(p) || /Windows/.test(ua)) return "windows";
    if (/mac|iphone|ipad|ipod/i.test(p) || /Mac OS X|iPhone|iPad/.test(ua)) return "macos";
    return "linux";
  }

  function initInstall() {
    var tabs = $$(".seg [role=tab]");
    if (!tabs.length) return;
    var panels = $$("[data-panel]");
    var script = $("[data-script]");
    var detected = $("[data-detected]");
    var copyBtn = $("[data-copy]");
    var copyLog = $("[data-copylog]");
    var os = detectOS();
    var timer = 0;

    function select(name, focus) {
      tabs.forEach(function (b) {
        var on = b.getAttribute("data-os") === name;
        b.setAttribute("aria-selected", on ? "true" : "false");
        b.tabIndex = on ? 0 : -1;
        if (on && focus) b.focus();
      });
      panels.forEach(function (p) {
        p.hidden = p.getAttribute("data-panel") !== name;
      });
      if (script) script.href = name === "windows" ? "install.ps1" : "install.sh";
    }

    tabs.forEach(function (b, i) {
      b.addEventListener("click", function () {
        select(b.getAttribute("data-os"));
      });
      b.addEventListener("keydown", function (e) {
        var j = -1;
        if (e.key === "ArrowRight") j = (i + 1) % tabs.length;
        else if (e.key === "ArrowLeft") j = (i - 1 + tabs.length) % tabs.length;
        else if (e.key === "Home") j = 0;
        else if (e.key === "End") j = tabs.length - 1;
        if (j < 0) return;
        e.preventDefault();
        select(tabs[j].getAttribute("data-os"), true);
      });
    });

    select(os);
    if (detected) {
      detected.textContent = "";
      fill(detected, ["detected ", fill(doc.createElement("b"), [os])]);
    }

    function write(text) {
      if (navigator.clipboard && window.isSecureContext) {
        return navigator.clipboard.writeText(text).catch(function () {
          return legacy(text);
        });
      }
      return legacy(text);
    }

    function legacy(text) {
      return new Promise(function (resolve, reject) {
        var t = doc.createElement("textarea");
        t.value = text;
        t.setAttribute("readonly", "");
        t.style.position = "fixed";
        t.style.opacity = "0";
        doc.body.appendChild(t);
        t.select();
        var ok = false;
        try {
          ok = doc.execCommand("copy");
        } catch (err) {
          ok = false;
        }
        doc.body.removeChild(t);
        if (ok) resolve();
        else reject(new Error("copy"));
      });
    }

    function log(level, label, msg) {
      copyLog.textContent = "";
      fill(copyLog, [span("lvl " + level, label), " ", span("ts", clock(true)), " " + msg]);
    }

    copyBtn.addEventListener("click", function () {
      var code = $("[data-panel]:not([hidden]) code");
      if (!code) return;
      write(code.textContent).then(
        function () {
          copyBtn.textContent = "copied";
          log("ok", "OK", "copied to clipboard");
          clearTimeout(timer);
          timer = setTimeout(function () {
            copyBtn.textContent = "copy";
          }, 1800);
        },
        function () {
          log("err", "ERR", "clipboard unavailable; select the command instead");
        }
      );
    });
  }

  function initNav() {
    var tabs = $$(".tabs .tab");
    var nav = $(".tabs");
    if (!tabs.length || !("IntersectionObserver" in window)) return;
    var watched = [doc.getElementById("top")].concat(
      tabs.map(function (t) {
        return doc.getElementById(t.getAttribute("href").slice(1));
      })
    ).filter(Boolean);

    function setOn(id) {
      tabs.forEach(function (t) {
        var on = t.getAttribute("href") === "#" + id;
        t.classList.toggle("on", on);
        if (on) {
          t.setAttribute("aria-current", "location");
          var l = t.offsetLeft;
          var r = l + t.offsetWidth;
          if (l < nav.scrollLeft) nav.scrollLeft = l;
          else if (r > nav.scrollLeft + nav.clientWidth) nav.scrollLeft = r - nav.clientWidth;
        } else {
          t.removeAttribute("aria-current");
        }
      });
    }

    var io = new IntersectionObserver(
      function (entries) {
        entries.forEach(function (e) {
          if (e.isIntersecting) setOn(e.target.id);
        });
      },
      { rootMargin: "-42% 0px -56% 0px" }
    );
    watched.forEach(function (s) {
      io.observe(s);
    });
  }

  function initStatus() {
    var bar = $(".statusline");
    if (!bar) return;
    var pos = $("[data-pos]", bar);
    var sclock = $("[data-sclock]", bar);
    var pend = $("[data-pend]", bar);
    var queued = false;
    var order = ["vault", "network", "fleet", "keyboard", "keys", "limits", "security", "more"];
    var pending = false;
    var pendTimer = 0;

    function update() {
      queued = false;
      var max = doc.documentElement.scrollHeight - window.innerHeight;
      var p = max > 0 ? window.scrollY / max : 0;
      pos.textContent = p < 0.01 ? "Top" : p > 0.99 ? "Bot" : Math.round(p * 100) + "%";
    }

    window.addEventListener(
      "scroll",
      function () {
        if (!queued) {
          queued = true;
          requestAnimationFrame(update);
        }
      },
      { passive: true }
    );
    update();

    function tick() {
      sclock.textContent = clock(false);
    }
    tick();
    setInterval(tick, 15000);

    function clear() {
      pending = false;
      pend.hidden = true;
      clearTimeout(pendTimer);
    }

    function go(id) {
      var el = id === "top" ? doc.getElementById("top") : doc.getElementById(id);
      if (!el) return;
      el.scrollIntoView({ behavior: reduced ? "auto" : "smooth", block: "start" });
      if (history.replaceState) history.replaceState(null, "", id === "top" ? location.pathname : "#" + id);
    }

    doc.addEventListener("keydown", function (e) {
      if (e.ctrlKey || e.metaKey || e.altKey || e.defaultPrevented) return;
      var t = e.target;
      if (t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName))) return;
      if (pending) {
        clear();
        if (/^[1-8]$/.test(e.key)) {
          e.preventDefault();
          go(order[Number(e.key) - 1]);
        } else if (e.key === "g") {
          e.preventDefault();
          go("top");
        }
        return;
      }
      if (e.key === "g") {
        pending = true;
        pend.hidden = false;
        pendTimer = setTimeout(clear, 1500);
      } else if (e.key === "G") {
        e.preventDefault();
        window.scrollTo({ top: doc.documentElement.scrollHeight, behavior: reduced ? "auto" : "smooth" });
      }
    });
  }

  function initDemo() {
    var demo = doc.getElementById("demo");
    if (!demo) return;
    var screen = $(".term-screen", demo);
    var lines = $("[data-lines]", demo);
    var toastSlot = $("[data-toast]", demo);
    var keycap = $("[data-keycap]", demo);
    var head = $("[data-head]", demo);
    var links = $("[data-links]", demo);
    var tunnels = $("[data-tunnels]", demo);
    var extra = $("[data-extra]", demo);
    var kbd = $("[data-kbd]", demo);
    var dclock = $("[data-clock]", demo);
    var pauseBtn = $("[data-pause]", demo);
    var tabs = $$(".ttab", demo);
    var STOP = { stop: true };
    var token = 0;
    var current = 0;
    var paused = false;
    var onscreen = true;
    var started = false;
    var waiters = [];

    function held() {
      return paused || !onscreen || doc.hidden;
    }

    function wake() {
      if (held()) return;
      var w = waiters;
      waiters = [];
      w.forEach(function (f) {
        f();
      });
    }

    function rand(a, b) {
      return a + Math.floor(Math.random() * (b - a + 1));
    }

    function trim() {
      var room = screen.clientHeight - 28;
      while (lines.children.length > 1 && lines.offsetHeight > room) lines.removeChild(lines.firstChild);
    }

    function row(cls, kids) {
      var d = doc.createElement("div");
      if (cls) d.className = cls;
      fill(d, kids || [" "]);
      lines.appendChild(d);
      trim();
      return d;
    }

    function sh(user, path) {
      return [span("t-user", user), ":", span("t-path", path), "$ "];
    }

    function rtt(v) {
      extra.textContent = "";
      fill(extra, ["rtt ", fill(doc.createElement("b"), [v])]);
    }

    function meter(v, label) {
      extra.textContent = "";
      var m = span("meter");
      var i = doc.createElement("i");
      i.style.setProperty("--v", String(v));
      m.appendChild(i);
      fill(extra, ["in flight ", m, fill(doc.createElement("b"), [label])]);
    }

    function setTab(i) {
      tabs.forEach(function (t, j) {
        t.setAttribute("aria-pressed", i === j ? "true" : "false");
      });
    }

    function reset(s) {
      lines.textContent = "";
      toastSlot.textContent = "";
      keycap.classList.remove("in");
      head.textContent = s.head;
      links.textContent = s.links;
      tunnels.textContent = s.tunnels;
      kbd.textContent = s.kbd;
      kbd.className = s.kbd === "us" ? "" : "t-amber";
      dclock.textContent = clock(false);
      rtt(s.rtt);
    }

    function ctx(t, fast) {
      function alive() {
        if (t !== token) throw STOP;
      }

      function sleep(ms) {
        if (fast) {
          alive();
          return Promise.resolve();
        }
        return new Promise(function (r) {
          setTimeout(r, ms);
        }).then(function loop() {
          alive();
          if (held()) {
            return new Promise(function (r) {
              waiters.push(r);
            }).then(loop);
          }
        });
      }

      async function prompt(parts, cmd, cls) {
        var text = span(cls || "t-fg");
        var cur = span("cursor");
        row("", parts.concat([text, cur]));
        await sleep(450);
        var chars = Array.from(cmd);
        if (fast) {
          text.textContent = cmd;
        } else {
          for (var i = 0; i < chars.length; i++) {
            text.textContent += chars[i];
            trim();
            await sleep(chars[i] === " " ? rand(60, 110) : rand(34, 82));
          }
        }
        await sleep(320);
        cur.remove();
      }

      function toast(level, msg, acts) {
        toastSlot.textContent = "";
        var el = doc.createElement("div");
        el.className = "ttoast" + (level === "ok" ? " ok" : "");
        fill(el, [span("lvl " + level, level === "ok" ? "OK" : "INFO"), span("ts", clock(true)), span("", msg)]);
        var primary = null;
        if (acts) {
          var a = span("acts");
          acts.forEach(function (x) {
            var b = span("btn small" + (x[1] ? " primary" : ""), x[0]);
            if (x[1]) primary = b;
            a.appendChild(b);
          });
          el.appendChild(a);
        }
        toastSlot.appendChild(el);
        if (fast) {
          el.classList.add("in");
        } else {
          requestAnimationFrame(function () {
            requestAnimationFrame(function () {
              el.classList.add("in");
            });
          });
        }
        return { el: el, primary: primary };
      }

      return { sleep: sleep, prompt: prompt, toast: toast, fast: fast, alive: alive };
    }

    var scenes = [
      {
        head: "fleet run · @prod · 5 hosts",
        links: "5/5",
        tunnels: "1",
        rtt: "41ms",
        kbd: "us",
        hold: 4200,
        run: async function (c) {
          await c.prompt([span("t-amber t-b", "›"), " "], "systemctl is-active nginx");
          row();
          var hosts = [
            ["web-1", 0, 38],
            ["web-2", 0, 41],
            ["web-3", 0, 44],
            ["db-1", 3, 52],
            ["edge-1", 0, 61]
          ];
          var cells = hosts.map(function (h) {
            var sq = span("sq warn blink");
            var ex = span("t-dim", "...");
            var tm = span("t-mute", "");
            row("fl", [span("t-fg", h[0]), sq, ex, tm, span("")]);
            return { h: h, sq: sq, ex: ex, tm: tm };
          });
          await c.sleep(260);
          var prev = 0;
          for (var i = 0; i < cells.length; i++) {
            var x = cells[i];
            await c.sleep((x.h[2] - prev) * 14 + 90);
            prev = x.h[2];
            x.sq.className = "sq " + (x.h[1] ? "err" : "ok");
            x.ex.className = x.h[1] ? "xn" : "x0";
            x.ex.textContent = "exit " + x.h[1];
            x.tm.textContent = x.h[2] + " ms";
          }
          await c.sleep(650);
          row();
          row("grp", [span("out", "active"), span("rule"), span("cnt", "4 hosts")]);
          await c.sleep(140);
          row("t-mute", ["  web-1  web-2  web-3  edge-1"]);
          await c.sleep(320);
          row("grp", [span("out", "inactive"), span("rule"), span("cnt", "1 host")]);
          await c.sleep(140);
          row("t-mute", ["  db-1"]);
          await c.sleep(420);
          row();
          row("", [span("lvl ok", "OK"), " ", span("ts", clock(true)), "  4 of 5 exited 0 · slowest 61 ms"]);
        }
      },
      {
        head: "deploy@web-1 · 10.0.4.21",
        links: "5/5",
        tunnels: "1",
        rtt: "38ms",
        kbd: "us",
        hold: 3600,
        run: async function (c) {
          await c.prompt(sh("deploy@web-1", "~/app"), "npm run dev");
          row();
          await c.sleep(260);
          row("t-mute", ["> app@1.4.0 dev"]);
          row("t-mute", ["> vite"]);
          await c.sleep(820);
          row();
          row("", ["  ", span("t-ok t-b", "VITE"), " ", span("t-ok", "v5.4.8"), "  ", span("t-dim", "ready in "), span("t-fg t-b", "312"), span("t-dim", " ms")]);
          row();
          row("", ["  ", span("t-ok", "->"), "  ", span("t-fg t-b", "Local:"), "   ", span("t-info", "http://localhost:5173/")]);
          row("", ["  ", span("t-ok", "->"), "  ", span("t-mute", "Network: use --host to expose")]);
          await c.sleep(1000);
          var t = c.toast("info", "port 5173 opened on web-1", [["forward and open", true], ["ignore", false]]);
          await c.sleep(1900);
          if (t.primary) t.primary.classList.add("press");
          await c.sleep(260);
          c.toast("ok", "127.0.0.1:5173 -> web-1:5173 · opened");
          tunnels.textContent = "2";
        }
      },
      {
        head: "sep@web-1 · layout fa",
        links: "5/5",
        tunnels: "2",
        rtt: "38ms",
        kbd: "fa",
        hold: 3800,
        run: async function (c) {
          await c.prompt(sh("sep@web-1", "~"), "echo \"سلام دنیا\"");
          await c.sleep(160);
          row("t-fg", ["سلام دنیا"]);
          await c.sleep(700);
          await c.prompt(sh("sep@web-1", "~"), "cat یادداشت.md");
          await c.sleep(160);
          row("t-fg t-b", ["# فهرست کارها"]);
          row("t-fg", ["- پشتیبان‌گیری از پایگاه داده"]);
          row("t-fg", ["- بررسی گواهی‌ها"]);
          await c.sleep(800);
          await c.prompt(sh("sep@web-1", "~"), "# typed on a persian layout, shaped rtl", "t-comment");
        }
      },
      {
        head: "ops@db-1 · 10.0.4.30",
        links: "5/5",
        tunnels: "2",
        rtt: "44ms",
        kbd: "us",
        hold: 3600,
        run: async function (c) {
          await c.prompt(sh("ops@db-1", "~"), "seq 1 100000000");
          var n = 0;
          var frames = c.fast ? 3 : 150;
          for (var f = 0; f < frames; f++) {
            n += c.fast ? 48210 : rand(900, 2400);
            for (var k = 0; k < 6; k++) {
              n += 1;
              row("t-fg", [String(n)]);
            }
            var v = 0.7 + Math.random() * 0.28;
            meter(v, (v * 4).toFixed(1) + " MiB");
            await c.sleep(16);
          }
          keycap.classList.add("in");
          await c.sleep(90);
          row("", ["^C"]);
          meter(0, "0.0 MiB");
          await c.sleep(500);
          keycap.classList.remove("in");
          await c.prompt(sh("ops@db-1", "~"), "# capped at 4 MiB in flight; ^C lands at once", "t-comment");
        }
      }
    ];

    async function play(i, fast) {
      var t = ++token;
      current = i;
      setTab(i);
      reset(scenes[i]);
      var c = ctx(t, fast);
      try {
        await scenes[i].run(c);
        if (fast) return;
        await c.sleep(scenes[i].hold);
        play((i + 1) % scenes.length, false);
      } catch (err) {
        if (err !== STOP) throw err;
      }
    }

    tabs.forEach(function (b, i) {
      b.addEventListener("click", function () {
        play(i, reduced);
      });
      b.addEventListener("keydown", function (e) {
        var j = -1;
        if (e.key === "ArrowRight") j = (i + 1) % tabs.length;
        else if (e.key === "ArrowLeft") j = (i - 1 + tabs.length) % tabs.length;
        else if (e.key === "Home") j = 0;
        else if (e.key === "End") j = tabs.length - 1;
        if (j < 0) return;
        e.preventDefault();
        tabs[j].focus();
        play(j, reduced);
      });
    });

    if (reduced) {
      pauseBtn.hidden = true;
      play(0, true);
      return;
    }

    pauseBtn.addEventListener("click", function () {
      paused = !paused;
      pauseBtn.textContent = paused ? "play" : "pause";
      pauseBtn.setAttribute("aria-pressed", paused ? "true" : "false");
      wake();
    });

    doc.addEventListener("visibilitychange", wake);

    if ("IntersectionObserver" in window) {
      new IntersectionObserver(
        function (entries) {
          onscreen = entries[0].isIntersecting;
          if (onscreen && !started) {
            started = true;
            play(0, false);
          }
          wake();
        },
        { threshold: 0.15 }
      ).observe(demo);
    } else {
      started = true;
      play(0, false);
    }
  }

  initInstall();
  initNav();
  initStatus();
  initDemo();
})();
