/* @ds-bundle: {"format":4,"namespace":"OpenDesktop","components":[{"name":"Logo"},{"name":"Button"},{"name":"Status"},{"name":"StageTable"},{"name":"VMRow"}]} */
(function () {
  "use strict";
  function h() { return window.React.createElement.apply(null, arguments); }
  function cx() { return Array.prototype.filter.call(arguments, Boolean).join(" "); }

  /* The mark: x86-64 variable-length bytes (top) ≡ ARM64 fixed 32-bit words (bottom). */
  var FULL = {
    box: "0 0 48 48", r: 1,
    top: [[8, 3], [13, 9], [24, 5], [31, 9]], topY: 11,
    mid: [8, 32], midY: 21.25,
    bottom: [[8, 6.5], [16.5, 6.5], [25, 6.5], [33.5, 6.5]], bottomY: 31.5,
    bar: 5.5
  };
  var SMALL = {
    box: "0 0 16 16", r: 0.5,
    top: [[2, 3], [6, 8]], topY: 3,
    mid: [2, 12], midY: 6.75,
    bottom: [[2, 2.25], [5.25, 2.25], [8.5, 2.25], [11.75, 2.25]], bottomY: 10.5,
    bar: 2.5
  };

  function Mark(props) {
    var size = props.size || 32;
    var g = size < 24 ? SMALL : FULL;
    var mid = props.tone === "mono" ? "od-mark-ink" : "od-mark-trace";
    function bars(list, y, cls) {
      return list.map(function (b, i) {
        return h("rect", { key: cls + i, x: b[0], y: y, width: b[1], height: g.bar, rx: g.r, className: cls });
      });
    }
    var attrs = { className: "od-mark", viewBox: g.box, width: size, height: size };
    if (props.decorative) attrs["aria-hidden"] = "true";
    else { attrs.role = "img"; attrs["aria-label"] = props.label || "Open Desktop"; }
    return h("svg", attrs,
      bars(g.top, g.topY, "od-mark-ink"),
      h("rect", { x: g.mid[0], y: g.midY, width: g.mid[1], height: g.bar, rx: g.r, className: mid }),
      bars(g.bottom, g.bottomY, "od-mark-ink"));
  }

  function Logo(props) {
    var size = props.size || 32;
    var lockup = props.lockup || "horizontal";
    if (lockup === "mark") return h("span", { className: cx("od-logo", props.className) }, h(Mark, { size: size, tone: props.tone }));
    return h("span", { className: cx("od-logo", props.className), style: { gap: Math.round(size * 0.25) + "px" } },
      h(Mark, { size: size, tone: props.tone, decorative: true }),
      h("span", { className: "od-logo-word", style: { fontSize: Math.round(size * 0.6) + "px" } }, "Open Desktop"));
  }

  function Button(props) {
    var variant = props.variant || "secondary";
    var rest = {};
    for (var k in props) if (k !== "variant" && k !== "size" && k !== "className") rest[k] = props[k];
    rest.className = cx("od-btn", "od-btn-" + variant, props.size === "sm" && "od-btn-sm", props.className);
    if (!rest.type) rest.type = "button";
    return h("button", rest, props.children);
  }

  var STATUS = {
    "accepted":    { tone: "pass", glyph: "≡", word: "Accepted" },
    "pass":        { tone: "pass", glyph: "≡", word: "Pass" },
    "fail":        { tone: "fail", glyph: "≢", word: "Fail" },
    "in-progress": { tone: "wip",  glyph: "≈", word: "In progress" },
    "partial":     { tone: "wip",  glyph: "≈", word: "Partial" },
    "early":       { tone: "wip",  glyph: "≈", word: "Early work" },
    "planned":     { tone: "plan", glyph: "○", word: "Planned" }
  };

  function Status(props) {
    var s = STATUS[props.status] || STATUS.planned;
    return h("span", { className: cx("od-status", "od-status-" + s.tone, props.className) },
      h("span", { className: "od-status-glyph", "aria-hidden": "true" }, s.glyph),
      h("span", null, props.children || s.word));
  }

  function StageTable(props) {
    var stages = props.stages || [];
    return h("div", { className: "od-table-box" },
      h("table", { className: "od-table" },
        props.caption ? h("caption", null, props.caption) : null,
        h("thead", null, h("tr", null, h("th", { scope: "col" }, "Stage"), h("th", { scope: "col" }, "Scope"), h("th", { scope: "col" }, "Status"))),
        h("tbody", null, stages.map(function (s) {
          return h("tr", { key: s.n },
            h("td", { className: "od-table-n" }, String(s.n)),
            h("td", null, s.scope),
            h("td", null, h(Status, { status: s.status }), s.note ? h("span", { className: "od-table-note" }, s.note) : null));
        }))));
  }

  var VM_STATE = {
    running: { tone: "pass", word: "Running" },
    stopped: { tone: "plan", word: "Stopped" },
    paused:  { tone: "wip",  word: "Paused" },
    error:   { tone: "fail", word: "Stopped with error" }
  };

  function VMRow(props) {
    var st = VM_STATE[props.state] || VM_STATE.stopped;
    var spec = [props.arch || "x86-64", props.cpus ? props.cpus + " vCPU" : null, props.memory || null].filter(Boolean).join(" · ");
    return h("div", { className: cx("od-vm", props.state === "error" && "od-vm-error", props.className) },
      h("div", { className: "od-vm-text" },
        h("span", { className: "od-vm-name" }, props.name),
        h("span", { className: "od-vm-spec" }, spec),
        props.detail ? h("span", { className: "od-vm-detail" }, props.detail) : null),
      h("span", { className: cx("od-status", "od-status-" + st.tone) }, h("span", { className: "od-status-dot", "aria-hidden": "true" }), h("span", null, st.word)),
      props.actions ? h("div", { className: "od-vm-actions" }, props.actions) : null);
  }

  window.OpenDesktop = { Logo: Logo, Button: Button, Status: Status, StageTable: StageTable, VMRow: VMRow };
})();
