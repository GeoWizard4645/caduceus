(function () {
  function mountBetaBanner() {
    if (document.getElementById("beta-banner") || !document.body) return;

    var bar = document.createElement("div");
    bar.id = "beta-banner";
    bar.className = "beta-banner";
    bar.setAttribute("role", "status");
    bar.setAttribute("aria-live", "polite");

    bar.innerHTML =
      '<p class="beta-banner__message">' +
      "<strong>Public beta</strong> — bugs and rough edges expected. Launch <strong>August 1, 2026</strong>. " +
      '<a href="https://github.com/GeoWizard4645/caduceus/issues">Report issues</a>.' +
      "</p>";

    document.body.insertBefore(bar, document.body.firstChild);
    document.body.classList.add("has-beta-banner");
  }

  if (document.body) mountBetaBanner();
  else document.addEventListener("DOMContentLoaded", mountBetaBanner);

  var KEY = "caduceus-site-theme";

  function current() {
    var t = document.documentElement.getAttribute("data-theme");
    if (t === "light" || t === "dark") return t;
    return "dark";
  }

  function apply(theme) {
    document.documentElement.setAttribute("data-theme", theme);
    try {
      localStorage.setItem(KEY, theme);
    } catch {
      /* private browsing */
    }
    var btn = document.getElementById("theme-toggle");
    if (btn) {
      btn.setAttribute(
        "aria-label",
        theme === "light" ? "Switch to dark mode" : "Switch to light mode",
      );
    }
  }

  window.caduceusSiteTheme = {
    toggle: function () {
      apply(current() === "light" ? "dark" : "light");
    },
  };

  document.addEventListener("DOMContentLoaded", function () {
    var btn = document.getElementById("theme-toggle");
    if (btn) {
      btn.addEventListener("click", window.caduceusSiteTheme.toggle);
      apply(current());
    }

    document.querySelectorAll('a[href*="github.com"]').forEach(function (link) {
      link.setAttribute("target", "_blank");
      var rel = link.getAttribute("rel") || "";
      if (rel.indexOf("noopener") === -1) {
        link.setAttribute(
          "rel",
          (rel ? rel + " " : "") + "noopener noreferrer",
        );
      }
    });
  });
})();
