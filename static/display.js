(() => {
  const POLL_INTERVAL_MS = 5 * 60 * 1000;

  const container = document.querySelector(".kiosk-week");
  if (!container) return;

  const token = new URLSearchParams(window.location.search).get("token") || "";

  function setField(dayEl, field, value) {
    const el = dayEl.querySelector(`[data-field="${field}"]`);
    if (!el) return;
    el.hidden = value === null || value === undefined;
    if (!el.hidden) el.textContent = value;
  }

  function patchDay(dayEl, day) {
    dayEl.classList.toggle("kiosk-day-today", day.is_today);

    setField(dayEl, "meal_name", day.meal_name);
    setField(dayEl, "empty", day.meal_name === null ? "Nothing planned" : null);
    setField(dayEl, "meal_time", day.meal_time);
    setField(dayEl, "attendees", day.attendees);
    setField(dayEl, "notes", day.notes);

    const statusEl = dayEl.querySelector('[data-field="sync_status"]');
    if (statusEl) {
      statusEl.hidden = day.sync_status === null || day.sync_status === undefined;
      if (!statusEl.hidden) {
        statusEl.textContent = day.sync_status;
        statusEl.className = `kiosk-sync-status kiosk-sync-status--${day.sync_status}`;
      }
    }
  }

  async function poll() {
    let response;
    try {
      response = await fetch(`/display/data?token=${encodeURIComponent(token)}`);
    } catch (err) {
      return; // transient network error - the next scheduled poll will retry
    }
    if (!response.ok) return;
    const data = await response.json();

    if (data.week_start !== container.dataset.weekStart) {
      // The tablet has been left up past a week rollover (or week_start_weekday
      // changed in Settings) - a full reload recomputes which 7 days to show.
      location.reload();
      return;
    }

    data.days.forEach((day, index) => {
      const dayEl = container.querySelector(`[data-day-index="${index}"]`);
      if (dayEl) patchDay(dayEl, day);
    });
  }

  setInterval(poll, POLL_INTERVAL_MS);
})();
