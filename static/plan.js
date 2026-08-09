(() => {
  const dialog = document.getElementById("meal-search-dialog");
  const searchInput = document.getElementById("meal-search-input");
  const resultsList = document.getElementById("meal-search-results");
  const closeBtn = document.getElementById("meal-search-close");
  if (!dialog) return;

  let activePicker = null;
  let currentItems = [];
  let highlightedIndex = -1;
  let debounceTimer = null;
  let requestToken = 0;

  function attendeeIdsFor(picker) {
    const form = picker.closest("form");
    return Array.from(form.querySelectorAll('input[name^="attendee_"]:checked'))
      .map((el) => el.name.slice("attendee_".length))
      .join(",");
  }

  function preferenceEmoji(item) {
    const liked = item.liked_by_attendee;
    const disliked = item.disliked_by_attendee_names && item.disliked_by_attendee_names.length > 0;
    if (liked && disliked) return "\u{1F937}"; // shrug - opinions are split
    if (disliked) return "\u{1F44E}"; // thumbs down
    if (liked) return "\u{1F44D}"; // thumbs up
    return "";
  }

  function renderResults(items, query) {
    resultsList.innerHTML = "";

    // Trigram similarity means "no exact match" often still returns similar
    // existing meals (e.g. typing a new "Chicken Curry" next to an existing
    // "Chicken Soup") - the create option must stay available whenever
    // there's no exact match, not just when there are zero results, or
    // there's no way to add a genuinely new but similar-sounding meal. It
    // goes first (not appended) so it's what Enter/the default highlight
    // picks - existing similar meals are demoted to "you might mean one of
    // these instead" rather than being silently auto-selected.
    const trimmedQuery = query.trim();
    const hasExactMatch = items.some(
      (item) => item.name.toLowerCase() === trimmedQuery.toLowerCase()
    );
    currentItems =
      trimmedQuery !== "" && !hasExactMatch
        ? [{ create: true, name: trimmedQuery }, ...items]
        : items;

    currentItems.forEach((item, index) => {
      const li = document.createElement("li");
      li.className = "meal-search-result";
      if (item.create) {
        li.textContent = `+ Create "${item.name}"`;
        li.classList.add("meal-search-create");
      } else {
        const label = document.createElement("span");
        label.className = "meal-search-label";

        const nameLine = document.createElement("span");
        let text = item.name;
        if (item.disliked_by_attendee_names && item.disliked_by_attendee_names.length > 0) {
          text += ` (disliked by ${item.disliked_by_attendee_names.join(", ")})`;
        }
        nameLine.textContent = text;
        label.appendChild(nameLine);

        if (item.last_planned) {
          const lastPlannedLine = document.createElement("span");
          lastPlannedLine.className = "meal-search-last-planned";
          lastPlannedLine.textContent =
            item.last_planned === "Never" ? "Never planned" : `Last planned ${item.last_planned}`;
          label.appendChild(lastPlannedLine);
        }

        li.appendChild(label);

        const emoji = preferenceEmoji(item);
        if (emoji) {
          const badge = document.createElement("span");
          badge.className = "meal-search-emoji";
          badge.textContent = emoji;
          li.appendChild(badge);
        }
      }
      li.addEventListener("mouseenter", () => setHighlight(index));
      li.addEventListener("click", () => commit(index));
      resultsList.appendChild(li);
    });

    setHighlight(currentItems.length > 0 ? 0 : -1);
  }

  function setHighlight(index) {
    const rows = resultsList.querySelectorAll(".meal-search-result");
    rows.forEach((row) => row.classList.remove("highlighted"));
    highlightedIndex = index;
    if (index >= 0 && rows[index]) {
      rows[index].classList.add("highlighted");
      rows[index].scrollIntoView({ block: "nearest" });
    }
  }

  async function fetchResults(query, attendeeIds) {
    const token = ++requestToken;
    const params = new URLSearchParams();
    if (query.trim() !== "") params.set("q", query.trim());
    if (attendeeIds) params.set("attendee_ids", attendeeIds);
    const response = await fetch(`/api/meals?${params.toString()}`);
    const items = await response.json();
    if (token !== requestToken) return; // a newer keystroke has already superseded this request
    renderResults(items, query);
  }

  // Every other field only auto-submits once a meal is chosen - otherwise
  // toggling an attendee or typing a note on an unplanned day would either
  // silently fail (meal_id is required) or pop the meal picker open on every
  // keystroke. Picking a meal is what "graduates" the day and saves
  // whatever's already been filled in alongside it.
  function autoSubmit(form) {
    const mealIdInput = form.querySelector('input[name="meal_id"]');
    if (!mealIdInput.value) return;
    form.requestSubmit();
  }

  function selectMeal(id, name) {
    if (!activePicker) return;
    activePicker.querySelector('input[name="meal_id"]').value = id;
    activePicker.querySelector(".meal-picker-trigger").textContent = name;
    dialog.close();
    activePicker.closest("form").requestSubmit();
  }

  async function createMeal(name) {
    const response = await fetch("/api/meals", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name }),
    });
    const meal = await response.json();
    selectMeal(meal.id, meal.name);
  }

  function commit(index) {
    const item = currentItems[index];
    if (!item) return;
    if (item.create) {
      createMeal(item.name);
    } else {
      selectMeal(item.id, item.name);
    }
  }

  function openPicker(picker) {
    activePicker = picker;
    const mealIdInput = picker.querySelector('input[name="meal_id"]');
    const currentName = picker.querySelector(".meal-picker-trigger").textContent.trim();
    // Leave the field blank (rather than pre-filled with the current meal) so
    // the full list is shown and typing searches fresh; the current
    // selection is shown as placeholder shadow-text instead, so it's still
    // visible but doesn't have to be cleared to pick something else, and
    // closing without typing leaves the day unchanged.
    searchInput.value = "";
    searchInput.placeholder = mealIdInput.value ? `Currently: ${currentName}` : "Search meals...";
    dialog.showModal();
    searchInput.focus();
    fetchResults("", attendeeIdsFor(picker));
  }

  document.addEventListener("click", (event) => {
    const trigger = event.target.closest(".meal-picker-trigger");
    if (trigger) {
      openPicker(trigger.closest(".meal-picker"));
      return;
    }

    // Same "Clear this day" action as the button at the bottom of the card -
    // just reachable right next to the meal you're trying to undo.
    const clearBtn = event.target.closest(".meal-picker-clear");
    if (clearBtn) {
      clearBtn.closest(".plan-day").querySelector("form.inline").requestSubmit();
      return;
    }

    const addGuestBtn = event.target.closest(".add-guest-btn");
    if (addGuestBtn) {
      addGuestField(addGuestBtn.previousElementSibling);
      return;
    }

    const removeGuestBtn = event.target.closest(".remove-guest-btn");
    if (removeGuestBtn) {
      const form = removeGuestBtn.closest("form");
      removeGuestBtn.closest(".guest-chip").remove();
      autoSubmit(form);
    }
  });

  // "change" (not "input") so text/number/time fields save once the user
  // moves on rather than on every keystroke; checkboxes fire "change"
  // immediately, which is the right behaviour for them.
  document.addEventListener("change", (event) => {
    const form = event.target.closest(".plan-day-form");
    if (form) autoSubmit(form);
  });

  closeBtn.addEventListener("click", () => dialog.close());
  dialog.addEventListener("click", (event) => {
    if (event.target === dialog) dialog.close();
  });

  searchInput.addEventListener("input", () => {
    clearTimeout(debounceTimer);
    const query = searchInput.value;
    const attendeeIds = activePicker ? attendeeIdsFor(activePicker) : "";
    debounceTimer = setTimeout(() => fetchResults(query, attendeeIds), 200);
  });

  searchInput.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setHighlight(Math.min(highlightedIndex + 1, currentItems.length - 1));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setHighlight(Math.max(highlightedIndex - 1, 0));
    } else if (event.key === "Enter") {
      event.preventDefault();
      commit(highlightedIndex);
    }
  });

  function addGuestField(guestList) {
    const nextIndex = Number(
      guestList.dataset.nextIndex || guestList.querySelectorAll("input").length
    );
    const chip = document.createElement("span");
    chip.className = "guest-chip";

    const input = document.createElement("input");
    input.type = "text";
    input.name = `guest_name_${nextIndex}`;
    input.placeholder = "Guest name";

    const removeBtn = document.createElement("button");
    removeBtn.type = "button";
    removeBtn.className = "remove-guest-btn";
    removeBtn.setAttribute("aria-label", "Remove guest");
    removeBtn.textContent = "×";

    chip.appendChild(input);
    chip.appendChild(removeBtn);
    guestList.appendChild(chip);
    guestList.dataset.nextIndex = String(nextIndex + 1);
    input.focus();
  }

  async function submitFormAjax(form) {
    const card = form.closest(".plan-day");
    const response = await fetch(form.action, {
      method: "POST",
      headers: { "X-Requested-With": "XMLHttpRequest" },
      body: new URLSearchParams(new FormData(form)),
    });
    if (!response.ok) return;
    const html = await response.text();
    activePicker = null;
    card.outerHTML = html;
  }

  // Delegated so it keeps working after a card is replaced via outerHTML - a
  // blank meal_id (native validation doesn't apply to hidden inputs) reopens
  // the picker instead of round-tripping to the server for a 422; every other
  // plan-day/clear-day submit is routed through AJAX instead of navigating.
  document.addEventListener("submit", (event) => {
    const form = event.target;
    const isPlanDayForm = form.matches(".plan-day-form");
    const isClearForm = form.matches(".plan-day form.inline");
    if (!isPlanDayForm && !isClearForm) return;

    // The clear form's onsubmit attribute (confirm() dialog) runs before this
    // delegated listener since it's attached directly to the form - if the
    // user cancelled, defaultPrevented is already true and the AJAX clear
    // below must not fire.
    if (event.defaultPrevented) return;

    if (isPlanDayForm) {
      const mealIdInput = form.querySelector('input[name="meal_id"]');
      if (!mealIdInput.value) {
        event.preventDefault();
        openPicker(form.querySelector(".meal-picker"));
        return;
      }
    }

    event.preventDefault();
    submitFormAjax(form);
  });
})();
