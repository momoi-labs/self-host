/* Theme selection. Three states, and "system" is the default.
   System means no data-theme attribute at all: the tokens are built on CSS
   light-dark(), so `color-scheme` on :root already follows the OS. Only an
   explicit choice is stored. */
const themeKey = 'console_theme';
const themes = {
  system: { label: 'System', icon: '<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="2" y="3" width="20" height="14" rx="2"></rect><path d="M8 21h8M12 17v4"></path></svg>' },
  light: { label: 'Light', icon: '<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="4"></circle><path d="M12 2v2M12 20v2M4.93 4.93l1.42 1.42M17.65 17.65l1.42 1.42M2 12h2M20 12h2M4.93 19.07l1.42-1.42M17.65 6.35l1.42-1.42"></path></svg>' },
  dark: { label: 'Dark', icon: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79Z"></path></svg>' },
};

function currentTheme() {
  const stored = localStorage.getItem(themeKey);
  return Object.hasOwn(themes, stored ?? "") ? stored : "system";
}

function applyTheme(theme) {
  if (theme === 'system') delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme;
  document.querySelectorAll('[data-theme-toggle] button').forEach(button => {
    button.setAttribute('aria-checked', String(button.dataset.themeValue === theme));
  });
}

/* Runs before the body parses, so the stored theme is on :root for the first
   paint and there is no flash of the wrong theme. */
applyTheme(currentTheme());

document.addEventListener('DOMContentLoaded', () => {
  document.querySelectorAll('[data-theme-toggle]').forEach(host => {
    host.innerHTML =
      '<span>Theme</span><div class="segmented" role="radiogroup" aria-label="Theme">' +
      Object.entries(themes).map(([value, { label, icon }]) =>
        `<button type="button" role="radio" data-theme-value="${value}" aria-checked="false" aria-label="${label} theme" title="${label}">${icon}</button>`
      ).join('') +
      '</div>';
    host.addEventListener('click', event => {
      const button = event.target.closest('[data-theme-value]');
      if (!button) return;
      const theme = button.dataset.themeValue;
      if (theme === 'system') localStorage.removeItem(themeKey);
      else localStorage.setItem(themeKey, theme);
      applyTheme(theme);
    });
  });
  applyTheme(currentTheme());
});
