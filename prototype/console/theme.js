const themeKey = 'console_theme';
const themeIcons = {
  light: '<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="4"></circle><path d="M12 2v2M12 20v2M4.93 4.93l1.42 1.42M17.65 17.65l1.42 1.42M2 12h2M20 12h2M4.93 19.07l1.42-1.42M17.65 6.35l1.42-1.42"></path></svg>',
  dark: '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79Z"></path></svg>',
};

function applyTheme(theme) {
  document.documentElement.toggleAttribute('data-theme', theme === 'light');
  if (theme === 'light') document.documentElement.dataset.theme = 'light';
  document.querySelectorAll('[data-theme-toggle]').forEach(button => {
    const nextTheme = theme === 'light' ? 'dark' : 'light';
    const label = `${nextTheme[0].toUpperCase()}${nextTheme.slice(1)} theme`;
    button.innerHTML = `${themeIcons[nextTheme]}<span>${label}</span>`;
    button.setAttribute('aria-label', `Use ${nextTheme} theme`);
    button.setAttribute('aria-pressed', String(theme === 'light'));
  });
}

function currentTheme() {
  const override = localStorage.getItem(themeKey);
  if (override === 'light' || override === 'dark') return override;
  return window.matchMedia?.('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
}

applyTheme(currentTheme());

document.addEventListener('DOMContentLoaded', () => {
  applyTheme(currentTheme());
  document.querySelectorAll('[data-theme-toggle]').forEach(button => {
    button.addEventListener('click', () => {
      const theme = currentTheme() === 'light' ? 'dark' : 'light';
      localStorage.setItem(themeKey, theme);
      applyTheme(theme);
    });
  });
});

window.matchMedia?.('(prefers-color-scheme: light)').addEventListener('change', event => {
  if (localStorage.getItem(themeKey) === null) applyTheme(event.matches ? 'light' : 'dark');
});
