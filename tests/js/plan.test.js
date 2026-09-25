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
  // hasMeal and hasEntry are deliberately separate: a day whose meal was
  // removed still owns its notes, attendees and time, and templates/macros.html
  // tracks that as has_entry rather than has_meal.
  hasMeal = true,
  hasEntry = true,
} = {}) {
  return `
<div class="card plan-day" data-date="${date}" data-planned="${hasEntry}">
    <h3>Saturday, 26 September</h3>
    <form method="post" action="/plan/${date}" class="plan-day-form">
        <input type="hidden" name="week_start" value="${date}">
        <div class="meal-picker">
            <input type="hidden" name="meal_id" value="${mealId}">
            <label>Meal
                <button type="button" class="meal-picker-trigger">${
                  hasMeal ? mealName : "Choose a meal"
                }</button>
                ${
                  hasMeal
                    ? `<button type="button" class="meal-picker-clear" aria-label="Remove meal">\u00d7</button>`
                    : ""
                }
            </label>
        </div>
        <div class="consumer-chips">
            <label class="chip">
                <input type="checkbox" name="attendee_1"${hasMeal ? " checked" : ""}>
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
        <button type="submit" class="secondary">${hasMeal ? "Reroll" : "Suggest"}</button>
    </form>
    ${
      hasMeal
        ? `<form method="post" action="/plan/${date}/clear-meal" class="plan-day-clear-meal-form" hidden onsubmit="return confirm('Remove the meal for this day? Everything else is kept.');">
        <input type="hidden" name="week_start" value="${date}">
    </form>`
        : ""
    }
    ${
      hasEntry
        ? `<form method="post" action="/plan/${date}/delete" class="plan-day-clear-form" onsubmit="return confirm('Clear this day? This cannot be undone.')">
        <input type="hidden" name="week_start" value="${date}">
        <button type="submit" class="secondary danger">Clear this day</button>
    </form>`
        : ""
    }
</div>`;
}

// The fixture for "the meal is gone but the day isn't" - the state the x leaves
// behind, and the one plan.js's data-planned gate exists for.
const MEAL_LESS_DAY = { hasMeal: false, mealId: "", mealName: "" };
const BRAND_NEW_DAY = { hasMeal: false, hasEntry: false, mealId: "", mealName: "" };

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
  // jsdom 26 ships no HTMLDialogElement.showModal at all - it's undefined, not
  // a no-op - so anything reaching the meal picker would throw. Stand in for
  // just the part plan.js uses, and keep `open` reflecting the attribute so a
  // test can assert the dialog was actually opened.
  if (!window.HTMLDialogElement.prototype.showModal) {
    window.HTMLDialogElement.prototype.showModal = function () {
      this.setAttribute("open", "");
    };
    window.HTMLDialogElement.prototype.close = function () {
      this.removeAttribute("open");
    };
  }
  const calls = installFetch(window);
  window.eval(PLAN_JS);
  return { window, document: window.document, calls };
}

// Let every queued microtask and awaited response handler run to completion.
const settle = () => new Promise((r) => setTimeout(r, 0));

// jsdom resolves form.action against the document URL, so compare paths.
const pathOf = (url) => new URL(url, "http://localhost:8080").pathname;

// The x removes the meal, not the day: the card has to come back with the
// picker empty but still carrying the user's notes, time and the explicit
// "Clear this day" action.
function assertMealRemoved(document, date, message) {
  const card = document.querySelector(`.plan-day[data-date="${date}"]`);
  assert.ok(card, `${message}: card still present`);
  assert.equal(
    card.querySelector(".meal-picker-clear"),
    null,
    `${message}: remove button still showing, so the meal never cleared`
  );
  assert.equal(
    card.querySelector(".meal-picker-trigger").textContent.trim(),
    "Choose a meal",
    `${message}: picker should fall back to its placeholder`
  );
  assert.equal(
    card.querySelector(".plan-day-suggest-form button").textContent.trim(),
    "Suggest",
    `${message}: suggest button still reads Reroll`
  );
  assert.equal(
    card.dataset.planned,
    "true",
    `${message}: removing the meal must not un-plan the day`
  );
  assert.ok(
    card.querySelector(".plan-day-clear-form"),
    `${message}: "Clear this day" must stay available on a meal-less day`
  );
}

test("the x removes just the meal and leaves the day planned", async () => {
  const { document, calls } = setup(cardHtml());

  document.querySelector(".meal-picker-clear").click();

  assert.equal(calls.length, 1);
  assert.equal(pathOf(calls[0].url), `/plan/${DATE}/clear-meal`);

  calls[0].respond(cardHtml({ ...MEAL_LESS_DAY, notes: "Family dinner" }));
  await settle();

  assertMealRemoved(document, DATE, "plain clear");
  assert.equal(
    document.querySelector('input[name="notes"]').value,
    "Family dinner",
    "notes around the removed meal should survive"
  );
});

test("the x no longer deletes the whole day", async () => {
  const { document, calls } = setup(cardHtml());

  document.querySelector(".meal-picker-clear").click();

  assert.notEqual(
    pathOf(calls[0].url),
    `/plan/${DATE}/delete`,
    "the whole-entry delete is a separate, explicitly labelled action"
  );
});

test("Clear this day still deletes the whole entry", async () => {
  const { document, calls } = setup(cardHtml());

  document.querySelector(".plan-day-clear-form button").click();

  assert.equal(calls.length, 1);
  assert.equal(pathOf(calls[0].url), `/plan/${DATE}/delete`);

  calls[0].respond(cardHtml({ hasMeal: false, hasEntry: false, mealId: "", mealName: "" }));
  await settle();

  const card = document.querySelector(`.plan-day[data-date="${DATE}"]`);
  assert.equal(card.dataset.planned, "false", "the day should be gone");
  assert.equal(
    card.querySelector(".plan-day-clear-form"),
    null,
    "a fully cleared day has nothing left to clear"
  );
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
for (const order of ["update-first", "clear-first"]) {
  test(`an autosave racing a remove leaves the meal gone (${order})`, async () => {
    const { window, document, calls } = setup(cardHtml());

    const notes = document.querySelector('input[name="notes"]');
    notes.value = "hello";
    notes.dispatchEvent(new window.Event("change", { bubbles: true }));
    document.querySelector(".meal-picker-clear").click();

    assert.equal(calls.length, 2, "expected an autosave and a clear");
    const [autosave, clear] = pathOf(calls[0].url).endsWith("/clear-meal")
      ? [calls[1], calls[0]]
      : [calls[0], calls[1]];
    assert.equal(pathOf(autosave.url), `/plan/${DATE}`);
    assert.equal(pathOf(clear.url), `/plan/${DATE}/clear-meal`);

    const stillPlanned = cardHtml({ notes: "hello" });
    const mealRemoved = cardHtml({ ...MEAL_LESS_DAY, notes: "hello" });
    if (order === "update-first") {
      autosave.respond(stillPlanned);
      await settle();
      clear.respond(mealRemoved);
    } else {
      clear.respond(mealRemoved);
      await settle();
      autosave.respond(stillPlanned);
    }
    await settle();

    assertMealRemoved(document, DATE, order);
  });
}

// data-planned is what separates "a day the user has already set up" from "a
// day that doesn't exist yet", and it gates autosave on both.
test("a meal-less day still autosaves its other fields", async () => {
  const { window, document, calls } = setup(cardHtml(MEAL_LESS_DAY));

  const notes = document.querySelector('input[name="notes"]');
  notes.value = "just a note";
  notes.dispatchEvent(new window.Event("change", { bubbles: true }));

  assert.equal(calls.length, 1, "a day with no meal is still a day and must autosave");
  assert.equal(pathOf(calls[0].url), `/plan/${DATE}`);
  assert.match(calls[0].body, /notes=just\+a\+note/);
  assert.match(calls[0].body, /meal_id=/);
});

test("a brand new day does not autosave before a meal is picked", () => {
  const { window, document, calls } = setup(cardHtml(BRAND_NEW_DAY));

  const notes = document.querySelector('input[name="notes"]');
  notes.value = "half-finished thought";
  notes.dispatchEvent(new window.Event("change", { bubbles: true }));

  assert.equal(
    calls.length,
    0,
    "an unplanned day has nothing to save yet - posting would create a meal-less row"
  );
});

test("submitting a brand new day with no meal reopens the picker", async () => {
  const { document, calls } = setup(cardHtml(BRAND_NEW_DAY));

  document.querySelector(".plan-day-form").requestSubmit();
  await settle();

  assert.ok(
    document.querySelector("#meal-search-dialog").open,
    "the user should be sent back to pick a meal"
  );
  assert.equal(
    calls.filter((c) => pathOf(c.url) === `/plan/${DATE}`).length,
    0,
    "the blank meal must be caught before the day is written"
  );
});

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
