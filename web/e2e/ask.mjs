// The app asks in a card of its own (web/src/ask.ts), not with confirm():
// answer every question with yes (or no) as soon as it shows.
export const autoAsk = (page, yes = true) =>
  page.addInitScript((yes) => {
    new MutationObserver(() => {
      const a = document.getElementById("ask");
      if (a && !a.hidden) document.getElementById(yes ? "ask-yes" : "ask-no")?.click();
    }).observe(document, { subtree: true, attributes: true, attributeFilter: ["hidden"] });
  }, yes);
