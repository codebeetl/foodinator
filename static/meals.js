(() => {
  const input = document.getElementById("add-meal-name");
  const duplicateNote = document.getElementById("add-meal-duplicate-note");
  const submitBtn = document.getElementById("add-meal-submit");
  const rows = Array.from(document.querySelectorAll("#meals-table tbody tr"));
  if (!input) return;

  function isExactMatch(value) {
    const normalized = value.trim().toLowerCase();
    return normalized !== "" && rows.some((row) => row.dataset.name.toLowerCase() === normalized);
  }

  function updateDuplicateState() {
    const duplicate = isExactMatch(input.value);
    duplicateNote.hidden = !duplicate;
    submitBtn.disabled = duplicate;
  }

  function filterTable(value) {
    const normalized = value.trim().toLowerCase();
    rows.forEach((row) => {
      row.hidden = normalized !== "" && !row.dataset.name.toLowerCase().includes(normalized);
    });
  }

  input.addEventListener("input", () => {
    filterTable(input.value);
    updateDuplicateState();
  });
})();
