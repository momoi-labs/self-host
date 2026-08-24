/* Theme selection. Three values, and "system" is the default.
   System means no data-theme attribute at all: the tokens are built on CSS
   light-dark(), so `color-scheme` on :root already follows the OS. Only an
   explicit choice narrows it. See kiso docs/components/theme-selector.md. */
const themeKey = 'kiso-theme';
const themes = {
  system: { name: 'Follow system', title: 'System', icon: 'i-monitor' },
  light: { name: 'Light theme', title: 'Light', icon: 'i-sun' },
  dark: { name: 'Dark theme', title: 'Dark', icon: 'i-moon' },
};

/* The three icons belong to this control, so it carries them itself rather
   than making every page that hosts a theme row remember to include them. */
const themeSprite = `
<svg width="0" height="0" style="position:absolute" aria-hidden="true"><defs>
<symbol id="i-monitor" viewBox="0 0 16 16"><rect x="1.75" y="2.75" width="12.5" height="8.5" rx="1.25"/><path d="M5.5 14h5M8 11.25V14"/></symbol>
<symbol id="i-sun" viewBox="0 0 16 16"><circle cx="8" cy="8" r="3"/><path d="M8 1v1.5M8 13.5V15M15 8h-1.5M2.5 8H1M12.9 3.1l-1 1M4.1 11.9l-1 1M12.9 12.9l-1-1M4.1 4.1l-1-1"/></symbol>
<symbol id="i-moon" viewBox="0 0 16 16"><path d="M13.5 9.5A5.75 5.75 0 016.5 2.5a5.75 5.75 0 107 7z"/></symbol>
</defs></svg>`;

/* Storage can be unavailable (private mode, disabled cookies). A failure has
   to degrade to "system", not throw and leave the page half-built. */
function currentTheme() {
  let stored = null;
  try { stored = localStorage.getItem(themeKey); } catch {}
  return Object.hasOwn(themes, stored ?? '') ? stored : 'system';
}

function storeTheme(theme) {
  try { localStorage.setItem(themeKey, theme); } catch {}
}

function applyTheme(theme) {
  if (theme === 'system') delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme;
  document.querySelectorAll('[data-theme-toggle] [data-theme-value]').forEach(button => {
    button.setAttribute('aria-selected', String(button.dataset.themeValue === theme));
  });
}

/* Runs before the body parses, so the stored theme is on :root for the first
   paint and there is no flash of the wrong theme. */
applyTheme(currentTheme());

document.addEventListener('DOMContentLoaded', () => {
  const hosts = document.querySelectorAll('[data-theme-toggle]');
  if (hosts.length) document.body.insertAdjacentHTML('afterbegin', themeSprite);
  hosts.forEach(host => {
    host.classList.add('settings-row');
    host.innerHTML =
      '<span class="t-label">Theme</span>' +
      '<div class="segmented" role="tablist" aria-label="Theme">' +
      Object.entries(themes).map(([value, { name, title, icon }]) =>
        `<button type="button" role="tab" data-theme-value="${value}" aria-selected="false"` +
        ` aria-label="${name}" title="${title}"><svg class="icon icon-sm"><use href="#${icon}"/></svg></button>`
      ).join('') +
      '</div>';
    host.addEventListener('click', event => {
      const button = event.target.closest('[data-theme-value]');
      if (!button) return;
      storeTheme(button.dataset.themeValue);
      applyTheme(button.dataset.themeValue);
    });
  });
  applyTheme(currentTheme());
});
