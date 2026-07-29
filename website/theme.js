(function () {
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
