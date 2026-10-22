/* GitMaster documentation: theme, mobile navigation, code copy, table of contents, search.
   No dependencies; everything the page needs is in this folder. */
(function () {
  "use strict";
  var doc = document.documentElement;

  /* Theme */
  var themeButton = document.getElementById("theme-toggle");
  function currentTheme() {
    return doc.dataset.theme || (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
  }
  if (themeButton) {
    themeButton.addEventListener("click", function () {
      var next = currentTheme() === "dark" ? "light" : "dark";
      doc.dataset.theme = next;
      try { localStorage.setItem("gm-theme", next); } catch (e) { /* storage may be blocked */ }
    });
  }

  /* Mobile navigation */
  var navToggle = document.querySelector(".nav-toggle");
  var scrim = document.querySelector(".nav-scrim");
  function setNav(open) {
    document.body.classList.toggle("nav-open", open);
    if (navToggle) navToggle.setAttribute("aria-expanded", String(open));
    if (scrim) scrim.hidden = !open;
  }
  if (navToggle) navToggle.addEventListener("click", function () { setNav(!document.body.classList.contains("nav-open")); });
  if (scrim) scrim.addEventListener("click", function () { setNav(false); });

  /* Keep the current page visible in the sidebar */
  var current = document.querySelector('.sidebar [aria-current="page"]');
  if (current && current.scrollIntoView) current.scrollIntoView({ block: "center" });

  /* Copy buttons on code blocks */
  document.querySelectorAll(".codeblock").forEach(function (block) {
    var code = block.querySelector("code");
    if (!code || !navigator.clipboard) return;
    var button = document.createElement("button");
    button.type = "button";
    button.className = "copy-button";
    button.textContent = "Copy";
    button.addEventListener("click", function () {
      navigator.clipboard.writeText(code.textContent).then(function () {
        button.textContent = "Copied";
        setTimeout(function () { button.textContent = "Copy"; }, 1500);
      });
    });
    block.appendChild(button);
  });

  /* Highlight the section being read in "On this page" */
  var tocLinks = Array.prototype.slice.call(document.querySelectorAll(".toc a"));
  if (tocLinks.length && "IntersectionObserver" in window) {
    var byId = {};
    tocLinks.forEach(function (a) { byId[decodeURIComponent(a.hash.slice(1))] = a; });
    var visible = {};
    var observer = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) { visible[entry.target.id] = entry.isIntersecting; });
      var first = Object.keys(byId).find(function (id) { return visible[id]; });
      if (first) {
        tocLinks.forEach(function (a) { a.removeAttribute("aria-current"); });
        byId[first].setAttribute("aria-current", "true");
      }
    }, { rootMargin: "-64px 0px -70% 0px" });
    Object.keys(byId).forEach(function (id) {
      var heading = document.getElementById(id);
      if (heading) observer.observe(heading);
    });
  }

  /* Search */
  var input = document.getElementById("search-input");
  var box = document.getElementById("search-results");
  if (!input || !box) return;
  var root = input.dataset.root || "";
  var index = null;
  var loading = null;
  var results = [];
  var selected = -1;

  function loadIndex() {
    if (index) return Promise.resolve(index);
    if (!loading) {
      loading = new Promise(function (resolve) {
        var script = document.createElement("script");
        script.src = input.dataset.script;
        script.onload = function () { index = window.GM_SEARCH || []; resolve(index); };
        script.onerror = function () { index = []; resolve(index); };
        document.head.appendChild(script);
      });
    }
    return loading;
  }

  function escapeHtml(text) {
    return text.replace(/[&<>"]/g, function (c) { return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]; });
  }

  function count(haystack, needle) {
    var n = 0;
    var at = haystack.indexOf(needle);
    while (at !== -1 && n < 5) { n++; at = haystack.indexOf(needle, at + needle.length); }
    return n;
  }

  function search(query) {
    var tokens = query.toLowerCase().split(/\s+/).filter(Boolean);
    if (!tokens.length) return [];
    var found = [];
    index.forEach(function (page) {
      var title = page.title.toLowerCase();
      var text = page.text.toLowerCase();
      var description = page.description.toLowerCase();
      var score = 0;
      var heading = null;
      for (var i = 0; i < tokens.length; i++) {
        var t = tokens[i];
        var here = 0;
        if (title.indexOf(t) === 0) here += 20; else if (title.indexOf(t) !== -1) here += 12;
        if (description.indexOf(t) !== -1) here += 2;
        for (var h = 0; h < page.headings.length; h++) {
          if (page.headings[h].text.toLowerCase().indexOf(t) !== -1) {
            here += 6;
            if (!heading) heading = page.headings[h];
            break;
          }
        }
        here += count(text, t);
        if (!here) return;
        score += here;
      }
      found.push({ page: page, score: score, heading: heading, tokens: tokens });
    });
    found.sort(function (a, b) { return b.score - a.score || a.page.title.localeCompare(b.page.title); });
    return found.slice(0, 8);
  }

  function snippet(page, tokens) {
    var lower = page.text.toLowerCase();
    var at = -1;
    for (var i = 0; i < tokens.length && at === -1; i++) at = lower.indexOf(tokens[i]);
    var text = at === -1 ? page.description : page.text.slice(Math.max(0, at - 60), at + 120);
    var html = escapeHtml(text);
    tokens.forEach(function (t) {
      html = html.replace(new RegExp("(" + t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") + ")", "ig"), "<mark>$1</mark>");
    });
    return (at > 60 ? "… " : "") + html + (at !== -1 ? " …" : "");
  }

  function render() {
    box.innerHTML = "";
    selected = -1;
    if (!input.value.trim()) { box.hidden = true; return; }
    if (!results.length) {
      box.innerHTML = '<div class="search-empty">No results</div>';
      box.hidden = false;
      return;
    }
    results.forEach(function (r, i) {
      var a = document.createElement("a");
      a.className = "search-result";
      a.setAttribute("role", "option");
      a.id = "search-option-" + i;
      a.href = root + r.page.url + (r.heading ? "#" + r.heading.id : "");
      a.innerHTML = '<span class="sr-title">' + escapeHtml(r.page.title) +
        (r.heading && r.heading.text !== r.page.title ? " › " + escapeHtml(r.heading.text) : "") +
        '<span class="sr-section">' + escapeHtml(r.page.section) + "</span></span>" +
        '<span class="sr-snippet">' + snippet(r.page, r.tokens) + "</span>";
      box.appendChild(a);
    });
    box.hidden = false;
  }

  function select(next) {
    var options = box.querySelectorAll(".search-result");
    if (!options.length) return;
    selected = (next + options.length) % options.length;
    options.forEach(function (o, i) { o.setAttribute("aria-selected", String(i === selected)); });
    input.setAttribute("aria-activedescendant", options[selected].id);
    options[selected].scrollIntoView({ block: "nearest" });
  }

  input.addEventListener("focus", loadIndex);
  input.addEventListener("input", function () {
    loadIndex().then(function () { results = search(input.value); render(); });
  });
  input.addEventListener("keydown", function (event) {
    if (event.key === "ArrowDown") { event.preventDefault(); select(selected + 1); }
    else if (event.key === "ArrowUp") { event.preventDefault(); select(selected - 1); }
    else if (event.key === "Enter") {
      var options = box.querySelectorAll(".search-result");
      var target = options[selected >= 0 ? selected : 0];
      if (target) { event.preventDefault(); window.location.href = target.href; }
    } else if (event.key === "Escape") { input.value = ""; box.hidden = true; input.blur(); }
  });
  document.addEventListener("click", function (event) {
    if (!box.contains(event.target) && event.target !== input) box.hidden = true;
  });
  document.addEventListener("keydown", function (event) {
    var tag = (event.target.tagName || "").toLowerCase();
    if (event.key === "/" && tag !== "input" && tag !== "textarea" && !event.metaKey && !event.ctrlKey) {
      event.preventDefault();
      input.focus();
    }
  });
})();
