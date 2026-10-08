# Login prototype

Compare three hierarchies for the same single-field API key flow. The Operator
preferred B and requested a full-height half-page help panel, hidden on mobile,
plus a monospaced serif font for the key. Layout preference is a hypothesis,
not a measured result.

B now places the current server origin below the brand, removes the visible
heading above the form, and uses Kiso's quiet Momoi watermark on theme paper
with a translucent form card. The accessible page heading remains available
to screen readers. The URL also appears under the mobile brand.

Start with `cd console && npm run prototype:login`. Open
http://127.0.0.1:5178/console/login.html?variant=B.

| Variant | Structure | Question |
| --- | --- | --- |
| A | Centered card with expandable help | Can the Operator enter a key with minimal reading? |
| B | Full-height help across the left half, hidden on mobile | Does visible key retrieval guidance prevent hesitation? |
| C | Open page with a horizontal form | Does an operational layout make connecting feel clearer? |

Use the bottom arrows to switch variants, or the left and right keyboard keys
outside fields. The URL retains the chosen variant. Switching variants or
submission scenarios resets the form. Theme changes stay in memory.

## Try it

1. Paste `sk-demo-operator-key`. Click Show, then Hide. The value stays intact.
2. Click Unlock console. Expect Checking key, a disabled submit button, then
   a simulated success message. The prototype does not navigate away.
3. Select Key rejected under On submit. Paste the demo value and submit.
   Expect a field error and focus on the key. The value remains available.
4. Repeat with Host offline and Host service failure. Expect connection or
   service instructions rather than a claim that the key is wrong.
5. Submit spaces only. Expect a request for a key without a pending state.
6. Compare the options in light and dark themes and at a narrow window width.
   Check that help and all controls remain reachable. In C, the form stacks.
   In B, the left panel takes half the desktop width and the available height.
   At 760px or narrower it disappears; the brand and expandable key help stay
   with the form. Click Show and compare `I`, `l`, `1`, `O` and `0` in the key.
   The revealed key uses Courier New, with Courier and monospace fallbacks,
   at the standard control size. Hide restores the normal masked-field font.
7. In B, confirm that the URL below self-host matches the current server origin,
   including protocol and port. Check the quiet watermark in both themes.

Ask which option makes it easiest to find a key, enter it and recover from a
failure. Compare the same tasks rather than asking which screen looks best.

## Kiso decision

The installed @momoi-labs/kiso-react version is 0.11.0. It has FormField's
suffix, hint and error slots, plus Button, Spinner, Alert and ThemeSelector.
Its exports do not include SecretInput or PasswordInput. This prototype
composes the reveal action from those existing controls without upgrading Kiso.
Button in this version has no dedicated loading prop, so the prototype combines
Spinner, a label and disabled state.

If the reveal interaction wins, propose a SecretInput that owns masking,
native input attributes, accessible reveal controls and FormField integration.
Self-host owns key retrieval guidance and request outcomes. This prototype does
not justify an AuthLayout or a complete LoginPage component.

Help uses self-host init --show-key to read the existing initial key on the
Host. An uninitialized Host needs self-host init first.

## Scope and validation

The existing login entry loads the prototype only in Vite development when
variant is present. Production builds retain the current login and remove the
prototype import. All prototype requests are timers. It sends no keys, stores
no keys. B shows the actual page origin; A and C use a fictional Host.
Autocomplete is off here to avoid filling
real credentials into a demo. Manager compatibility must be evaluated on the
actual console origin during implementation.

Validation uses TypeScript, the production build and HTTP checks of the Vite
entry and transformed modules. Browser automation and visual inspection were
not performed, following the user's instruction. These checks do not establish
layout quality, keyboard behavior or screen reader behavior.

The prototype is excluded from the cargo-served production bundle, so its
scenarios cannot be tested through cargo run -- serve. They need no backend.
Real-backend authentication tests belong to the selected implementation.
Keep this code temporary until a variant is chosen, then implement the result
and capture the alternatives on a throwaway branch.
