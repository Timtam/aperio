// Language switcher for the Aperio user guide.
//
// The German guide (docs/user, served at /aperio/user/) and the English
// guide (docs/user-en, served at /aperio/user-en/) share an identical file
// tree, so the counterpart of any page is the same path with the book
// segment swapped. This injects a link into the mdBook menu bar that keeps
// you on the same chapter when switching languages.
(function () {
  "use strict";

  function counterpart(pathname) {
    if (pathname.indexOf("/user-en/") !== -1) {
      // Currently on the English guide -> link to German.
      return {
        href: pathname.replace("/user-en/", "/user/"),
        // lang/text describe the TARGET language in its own language.
        lang: "de",
        text: "Deutsch",
        // aria-label is in the CURRENT page language (English here).
        ariaLabel: "Switch language to German",
      };
    }
    if (pathname.indexOf("/user/") !== -1) {
      // Currently on the German guide -> link to English.
      return {
        href: pathname.replace("/user/", "/user-en/"),
        lang: "en",
        text: "English",
        ariaLabel: "Sprache zu Englisch wechseln",
      };
    }
    return null;
  }

  function inject() {
    var info = counterpart(window.location.pathname);
    if (!info) {
      return;
    }

    var menuBar = document.getElementById("menu-bar");
    if (!menuBar) {
      return;
    }
    var container = menuBar.querySelector(".right-buttons") || menuBar;

    var link = document.createElement("a");
    link.href = info.href;
    link.textContent = info.text;
    link.setAttribute("lang", info.lang);
    link.setAttribute("hreflang", info.lang);
    link.setAttribute("aria-label", info.ariaLabel);
    link.setAttribute("title", info.ariaLabel);
    link.className = "lang-switcher";
    link.style.padding = "0 8px";
    link.style.fontWeight = "600";
    link.style.whiteSpace = "nowrap";

    container.appendChild(link);
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", inject);
  } else {
    inject();
  }
})();
