// Browser-side regression tests for static/plan.js.
//
// The Rust suite can't reach any of this: the bug this file exists for was a
// client-side race (two writes for one card, the slower response painting a
// detached node) that every server test passed straight through.
//
// The card fixture below mirrors templates/macros.html's plan_day_card. If a
// hook class or the data-date identity attribute is renamed there, update it
// here too - test_plan_day_card_exposes_js_hooks in src/web/plan.rs guards the
// server-rendered half of the same contract.

const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { JSDOM } = require("jsdom");

const PLAN_JS = readFileSync(require.resolve("../../static/plan.js"), "utf8");

const DATE = "2026-09-26";
const OTHER_DATE = "2026-09-27";

function cardHtml({
  date = DATE,
  mealId = "1",
  mealName = "Tacos",
  notes = "",
  planned = true,
} = {}) {
  return `
<div class="card plan-day" data-date="${date}">
    <h3>Saturday, 26 September</h3>
    <form method="post" action="/plan/${date}" class="plan-day-form">
        <input type="hidden" name="week_start" value="${date}">
        <div class="meal-picker">
            <input type="hidden" name="meal_id" value="${mealId}">
            <label>Meal
                <button type="button" class="meal-picker-trigger">${
                  planned ? mealName : "Choose a meal"
                }</button>
                ${
                  planned
                    ? `<button type="button" class="meal-picker-clear" aria-label="Clear this day">\u00d7</button>`
                    : ""
                }
            </label>
        </div>
        <div class="consumer-chips">
            <label class="chip">
                <input type="checkbox" name="attendee_1"${planned ? " checked" : ""}>
                Alice
            </label>
        </div>
        <div class="guest-list"></div>
        <button type="button" class="secondary add-guest-btn">+ Add guest</button>
        <label>Notes <input type="text" name="notes" value="${notes}"></label>
        <label>Meal time
            <input type="time" name="meal_time" value='18:30'>
        </label>
        <label>Duration (minutes)
            <input type="number" name="duration_minutes" min="1" value="30">
        </label>
    </form>
    <form method="post" action="/plan/${date}/suggest" class="plan-day-suggest-form inline">
        <input type="hidden" name="week_start" value="${date}">
        <button type="submit" class="secondary">${planned ? "Reroll" : "Suggest"}</button>
    </form>
    ${
      planned
        ? `<form method="post" action="/plan/${date}/delete" class="plan-day-clear-form" onsubmit="return confirm('Clear this day? This cannot be undone.')">
        <input type="hidden" name="week_start" value="${date}">
        <button type="submit" class="secondary danger">Clear this day</button>
    </form>`
        : ""
    }
</div>`;
}

const DIALOG = `
<dialog id="meal-search-dialog">
    <input type="text" id="meal-search-input">
    <ul id="meal-search-results"></ul>
    <button type="button" id="meal-search-close">Close</button>
</dialog>`;

// Stubbed fetch: records each call and hands back a promise only the test
// settles, so a test can decide which response lands first and in what order.
function installFetch(window) {
  const calls = [];
  window.fetch = (url, init) => {
    let resolve;
    const response = new Promise((r) => {
      resolve = r;
    });
    calls.push({
      url: String(url),
      method: (init && init.method) || "GET",
      body: init && init.body ? String(init.body) : null,
      respond(html, status = 200) {
        resolve({
          ok: status < 400,
          status,
          text: () => Promise.resolve(html),
        });
      },
    });
    return response;
  };
  return calls;
}

function setup(bodyHtml) {
  const dom = new JSDOM(
    `<!doctype html><html><body>${bodyHtml}${DIALOG}</body></html>`,
    {
      // "dangerously" so inline onsubmit="" attributes (the clear form's
      // confirm guard) are compiled - "outside-only" skips them.
      runScripts: "dangerously",
      url: "http://localhost:8080/plan",
    }
  );
  const { window } = dom;
  window.confirm = () => true;
  const calls = installFetch(window);
  window.eval(PLAN_JS);
  return { window, document: window.document, calls };
}

// Let every queued microtask and awaited response handler run to completion.
const settle = () => new Promise((r) => setTimeout(r, 0));

// jsdom resolves form.action against the document URL, so compare paths.
const pathOf = (url) => new URL(url, "http://localhost:8080").pathname;

function assertCardCleared(document, date, message) {
  const card = document.querySelector(`.plan-day[data-date="${date}"]`);
  assert.ok(card, `${message}: card still present`);
  assert.equal(
    card.querySelector(".meal-picker-clear"),
    null,
    `${message}: clear button still showing, so the meal never cleared`
  );
  assert.equal(
    card.querySelector(".plan-day-suggest-form button").textContent.trim(),
    "Suggest",
    `${message}: suggest button still reads Reroll`
  );
}

test("clearing a day posts to the delete endpoint and repaints the card as cleared", async () => {
  const { document, calls } = setup(cardHtml());

  document.querySelector(".meal-picker-clear").click();

  assert.equal(calls.length, 1);
  assert.equal(pathOf(calls[0].url), `/plan/${DATE}/delete`);

  calls[0].respond(cardHtml({ planned: false, mealId: "", mealName: "" }));
  await settle();

  assertCardCleared(document, DATE, "plain clear");
});

test("cancelling the confirm dialog leaves the day alone", () => {
  const { window, document, calls } = setup(cardHtml());
  window.confirm = () => false;

  document.querySelector(".meal-picker-clear").click();

  assert.equal(calls.length, 0, "a cancelled clear must not reach the server");
  assert.ok(document.querySelector(".meal-picker-clear"), "card should be untouched");
});

// The regression this file exists for: editing a field and then clicking the x
// issues two writes for the same card (the blur's change event autosaves, the
// click clears). Whichever response lands first replaces the card, so the
// other one is holding a detached node - and the day used to end up showing a
// meal the server had already cleared. The clear is always issued last, so it
// must win regardless of the order the responses come back in.
for (const order of ["update-first", "delete-first"]) {
  test(`an autosave racing a clear leaves the day cleared (${order})`, async () => {
    const { window, document, calls } = setup(cardHtml());

    const notes = document.querySelector('input[name="notes"]');
    notes.value = "hello";
    notes.dispatchEvent(new window.Event("change", { bubbles: true }));
    document.querySelector(".meal-picker-clear").click();

    assert.equal(calls.length, 2, "expected an autosave and a clear");
    const [autosave, clear] = pathOf(calls[0].url).endsWith("/delete")
      ? [calls[1], calls[0]]
      : [calls[0], calls[1]];
    assert.equal(pathOf(autosave.url), `/plan/${DATE}`);
    assert.equal(pathOf(clear.url), `/plan/${DATE}/delete`);

    const stillPlanned = cardHtml({ notes: "hello" });
    const cleared = cardHtml({ planned: false, mealId: "", mealName: "" });
    if (order === "update-first") {
      autosave.respond(stillPlanned);
      await settle();
      clear.respond(cleared);
    } else {
      clear.respond(cleared);
      await settle();
      autosave.respond(stillPlanned);
    }
    await settle();

    assertCardCleared(document, DATE, order);
  });
}

// A newer request for a *different* day must not cancel an in-flight repaint -
// this is what keeps the sequencing per-day rather than global.
test("writes to two different days both repaint", async () => {
  const { document, calls } = setup(
    cardHtml() + cardHtml({ date: OTHER_DATE, mealId: "2", mealName: "Curry" })
  );

  document
    .querySelector(`.plan-day[data-date="${DATE}"] .plan-day-suggest-form button`)
    .click();
  document
    .querySelector(`.plan-day[data-date="${OTHER_DATE}"] .plan-day-suggest-form button`)
    .click();
  assert.equal(calls.length, 2);

  calls[0].respond(cardHtml({ mealId: "3", mealName: "Pasta" }));
  await settle();
  calls[1].respond(cardHtml({ date: OTHER_DATE, mealId: "4", mealName: "Salad" }));
  await settle();

  assert.equal(
    document.querySelector(`.plan-day[data-date="${DATE}"] .meal-picker-trigger`).textContent.trim(),
    "Pasta"
  );
  assert.equal(
    document
      .querySelector(`.plan-day[data-date="${OTHER_DATE}"] .meal-picker-trigger`)
      .textContent.trim(),
    "Salad"
  );
});
