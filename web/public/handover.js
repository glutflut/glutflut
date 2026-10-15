// Domain move elements-game.de -> glutflut.com (2026-10). Old domain: take
// this browser's game state (localStorage "elements.*": token, permanent
// source tickets, language, city, consents) along once in the URL fragment,
// which never reaches a server, and stop this page before the app starts.
// New domain: store what isn't there yet, then clean the address bar.
// A classic script first in <head>, before the app's module scripts.
(function () {
  var OLD = "elements-game.de";
  var NEW = "glutflut.com";
  var TAG = "#handover=";
  // Staff tokens are typed in again; caches refill.
  var SKIP = { "elements.ops": 1, "elements.chapters": 1, "elements.reloaded-for": 1 };
  var host = location.hostname;
  try {
    if (host === OLD || host.slice(-OLD.length - 1) === "." + OLD) {
      var data = {};
      for (var i = 0; i < localStorage.length; i++) {
        var k = localStorage.key(i);
        if (k && k.indexOf("elements.") === 0 && !SKIP[k]) data[k] = localStorage.getItem(k);
      }
      var json = JSON.stringify(data);
      var to = "https://" + host.slice(0, host.length - OLD.length) + NEW + location.pathname + location.search;
      if (json !== "{}") to += TAG + encodeURIComponent(btoa(unescape(encodeURIComponent(json))));
      location.replace(to);
      // The rest of the page becomes inert text: no app, no socket, no new token.
      document.write('<plaintext style="display:none">');
      return;
    }
    if (location.hash.indexOf(TAG) === 0) {
      var got = JSON.parse(decodeURIComponent(escape(atob(decodeURIComponent(location.hash.slice(TAG.length))))));
      for (var key in got) {
        if (key.indexOf("elements.") === 0 && !SKIP[key] && localStorage.getItem(key) === null) {
          localStorage.setItem(key, String(got[key]));
        }
      }
      history.replaceState(history.state, "", location.pathname + location.search);
    }
  } catch (e) {
    // Storage blocked or a broken fragment: start fresh on the new domain.
  }
})();
