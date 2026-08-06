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
    currentItems = items;
    resultsList.innerHTML = "";

    if (items.length === 0 && query.trim() !== "") {
      currentItems = [{ create: true, name: query.trim() }];
    }

    currentItems.forEach((item, index) => {
      const li = document.createElement("li");
      li.className = "meal-search-result";
      if (item.create) {
        li.textContent = `+ Create "${item.name}"`;
        li.classList.add("meal-search-create");
      } else {
        const label = document.createElement("span");
        let text = item.name;
        if (item.disliked_by_attendee_names && item.disliked_by_attendee_names.length > 0) {
          text += ` (disliked by ${item.disliked_by_attendee_names.join(", ")})`;
        }
        label.textContent = text;
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
    searchInput.value = "";
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

  // A blank meal_id (native validation doesn't apply to hidden inputs) should
  // reopen the picker instead of round-tripping to the server for a 422.
  document.querySelectorAll(".plan-day-form").forEach((form) => {
    form.addEventListener("submit", (event) => {
      const mealIdInput = form.querySelector('input[name="meal_id"]');
      if (!mealIdInput.value) {
        event.preventDefault();
        openPicker(form.querySelector(".meal-picker"));
      }
    });
  });
})();
