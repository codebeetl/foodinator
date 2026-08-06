(() => {
  const input = document.getElementById("add-meal-name");
  const suggestions = document.getElementById("add-meal-suggestions");
  const duplicateNote = document.getElementById("add-meal-duplicate-note");
  const submitBtn = document.getElementById("add-meal-submit");
  if (!input) return;

  let debounceTimer = null;
  let requestToken = 0;
  let currentNames = [];

  function isExactMatch(value) {
    const normalized = value.trim().toLowerCase();
    return normalized !== "" && currentNames.some((name) => name.toLowerCase() === normalized);
  }

  function updateDuplicateState() {
    const duplicate = isExactMatch(input.value);
    duplicateNote.hidden = !duplicate;
    submitBtn.disabled = duplicate;
  }

  function renderSuggestions(names) {
    suggestions.innerHTML = "";
    if (names.length === 0) {
      suggestions.hidden = true;
      return;
    }
    names.forEach((name) => {
      const li = document.createElement("li");
      li.textContent = name;
      li.addEventListener("click", () => {
        input.value = name;
        suggestions.hidden = true;
        updateDuplicateState();
        input.focus();
      });
      suggestions.appendChild(li);
    });
    suggestions.hidden = false;
  }

  async function fetchSuggestions(query) {
    const token = ++requestToken;
    const params = new URLSearchParams({ q: query });
    const response = await fetch(`/api/meals?${params.toString()}`);
    const items = await response.json();
    if (token !== requestToken) return; // a newer keystroke has already superseded this request
    currentNames = items.map((item) => item.name);
    renderSuggestions(currentNames);
    updateDuplicateState();
  }

  input.addEventListener("input", () => {
    clearTimeout(debounceTimer);
    const query = input.value.trim();
    if (query === "") {
      currentNames = [];
      renderSuggestions([]);
      updateDuplicateState();
      return;
    }
    debounceTimer = setTimeout(() => fetchSuggestions(query), 200);
  });

  document.addEventListener("click", (event) => {
    if (!suggestions.contains(event.target) && event.target !== input) {
      suggestions.hidden = true;
    }
  });
})();
