# Login usability and Kiso assessment

Keep the current single-field API key flow and fix credential entry, feedback,
and access instructions first. A reusable revealable secret input is the
strongest candidate for Kiso. A new authentication method needs a separate
product decision.

Research date: 2026-10-08. This is an assessment, not an implementation plan.

## Approved implementation

The Operator approved variant B after comparing the interactive alternatives.
The [prototype branch](https://github.com/momoi-labs/self-host/tree/prototype/login-variants-20261008)
preserves the throwaway prototype and the decision. The real login uses the split layout with a
full-height help panel occupying half the desktop width, hidden on mobile.
The mobile form keeps the brand, server origin and expandable key help.
The key uses a standard-size monospaced serif font only when revealed.
The background uses Kiso's quiet Momoi watermark and a translucent card.

Production authentication still checks /health, stores the accepted key in
sessionStorage and opens /console/. It adds pending protection, key validation
and separate authentication, service and network errors. Key help now uses
self-host init --show-key. No Kiso dependency upgrade or new upstream component
was needed; the reveal control composes FormField's suffix and Button.

[Kiso issue #173](https://github.com/momoi-labs/blueprint/issues/173) proposes
aligning the gallery example and authentication pattern with this direction.
The self-host [add-page login checklist](../../skills/add-page/login.md)
records the implemented product rules.

Implementation checks passed: TypeScript, the console production build, all 64
console tests, the full Rust test suite and clippy with warnings denied. An isolated instance
started through cargo run -- serve --no-dns at http://127.0.0.1:3748/console.
HTTP checks verified that its assets match the rebuilt bundle, invalid keys
return 401 and the disposable key returns 200. Init --show-key also returned
only the fixture key while the daemon held the state open. The fixture uses
temporary Platform State and cannot reach Docker. Browser interactions and
visual checks remain manual, following the Operator's instruction.

The sections below record the original assessment before implementation.

## Project context and current behavior

The Platform has one Operator and should work on the LAN without an external
service on the happy path. See [CONTEXT.md](../../CONTEXT.md). That makes
consumer SaaS login examples a poor default for choosing authentication.

I read [login.tsx](../../console/src/login.tsx),
[api.ts](../../console/src/lib/api.ts), and
[package.json](../../console/package.json). I also inspected the login served
at http://127.0.0.1:3747/console, which displayed version 0.5.0. I could not
verify which worktree started that process. The code findings below refer to
this worktree, rather than assuming the running bundle matches it.

The login has a visible API key label, a masked field, initial focus, a native
form, one primary action, an error alert, and a theme selector. These are useful
parts to preserve.

| Finding in the code                                     | Effect on the Operator                                                   | Recommendation                                                                    |
| ------------------------------------------------------- | ------------------------------------------------------------------------ | --------------------------------------------------------------------------------- |
| No reveal control                                       | A long key is difficult to check visually                                | Offer Show API key and Hide API key                                               |
| No pending state or submission guard                    | Repeated clicks can send concurrent requests                             | Show Checking key and prevent duplicate submission                                |
| Every non-success HTTP response becomes a key rejection | An unavailable Host can look like a bad credential                       | Distinguish authentication refusal, service failure, and connection failure       |
| The catch branch displays the exception message         | A browser network error can bypass the friendly fallback                 | Map network failures to a clear action the Operator can take                      |
| autocomplete is off                                     | The field does not communicate an existing credential purpose            | Evaluate current-password and test actual manager behavior                        |
| The only help says to run self-host serve               | It does not say where to run it or what to do when the Host already runs | Explain the supported retrieval or recovery procedure on the Host                 |
| Successful entry stores the key in sessionStorage       | Access state belongs to the tab session                                  | Explain the behavior accurately; treat persistence changes as a separate decision |

The form does not implement a paste blocker. Keep paste available. Do not infer
a WCAG failure from autocomplete=off alone: password managers can ignore that
attribute, and copy/paste is also an assistance mechanism. Test interoperability.

## What the sources support

### Accessible authentication

[WCAG 2.2, understanding SC 3.3.8](https://www.w3.org/WAI/WCAG22/Understanding/accessible-authentication-minimum.html)
explains the AA requirement to avoid unsupported cognitive tests during
authentication. Password managers and copy/paste are examples of assistance.
It also discusses optional reveal controls as help for people who have trouble
checking hidden characters. This supports reducing transcription and memory
burden. It does not prescribe a centered card or prove that a decorative
layout improves completion. The Understanding document explains the standard;
it is not itself the normative WCAG text.

### A concrete interaction precedent

[GOV.UK password input](https://design-system.service.gov.uk/components/password-input/)
provides a show/hide control, masking by default, credential autocomplete, and
copy/paste guidance. It also addresses spellcheck and automatic capitalization
when values become visible. This is a practical design-system precedent, not
a controlled comparison of layouts. Its scope is passwords, so applying the
interaction to an API key is our design judgment. Keep the label specific to
the API key instead of pretending it is an account password.

### Comparing authentication options

[Bonneau, Herley, van Oorschot and Stajano, 2012](https://www.microsoft.com/en-us/research/publication/the-quest-to-replace-passwords-a-framework-for-comparative-evaluation-of-web-authentication-schemes/)
compare schemes across 25 usability, deployability, and security benefits.
The framework is useful for asking what an alternative costs as well as what
it improves. I consulted the institutional publication page and abstract,
not the complete paper. The study predates current passkey deployments.

### Passkey deployment constraints

[WebAuthn Level 3](https://www.w3.org/TR/webauthn-3/#sctn-api)
scopes public-key credentials to a relying party and requires a secure
context. For this Platform, passkeys would require investigating LAN naming,
trusted transport, enrollment, and recovery. A successful localhost experiment
would not establish that the intended LAN deployment works.

### Visual references

[Landingfolio login gallery](https://www.landingfolio.com/inspiration/login)
is a source of curated visual examples.
[Mobbin login screens](https://mobbin.com/explore/web/screens/login) is a
user-provided reference that I did not inspect in this session. Neither a
screenshot nor a popular layout establishes keyboard access, error recovery,
or successful credential entry. Use examples to generate alternatives and
then evaluate the complete interaction.

## Layout choices

These are recommendations for this project, not measured results.

| Option                  | When it makes sense                                         | Assessment for self-host                                                                                       |
| ----------------------- | ----------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| One focused column      | One credential and one action                               | Recommended baseline. Retain the current structure and improve help and feedback                               |
| Form plus a help column | Operators need substantial Host or recovery instructions    | A possible desktop variant. The second column should answer a real access question and stack on narrow screens |
| Multi-step entry        | A real discovery, pairing, or identity-provider step exists | No reason to add a step to the current API key flow                                                            |

The [add-page skill](../../skills/add-page/SKILL.md) explicitly excludes login
from the list, detail, and create layouts. Its checklist and
[ADR-0024](../adr/0024-detail-and-list-screens-share-one-layout.md) do not
justify adding a sidebar, lifecycle row, or sticky save bar to this screen.

## Authentication choices

This comparison applies the paper's tradeoff framework to the project. It is
a contextual assessment, not a claim that the paper evaluated this Platform.

| Option                                       | Potential gain                                                | Cost or open question                                                 | Recommendation                                                  |
| -------------------------------------------- | ------------------------------------------------------------- | --------------------------------------------------------------------- | --------------------------------------------------------------- |
| Current API key                              | One field and no identity-provider dependency                 | Retrieving and repeatedly entering a long secret                      | Improve this flow first                                         |
| Local browser pairing and a separate session | Could reduce repeated use of an API credential in the browser | Requires approval, session lifecycle, revocation, and recovery design | Investigate only if repeated key entry remains the main problem |
| Passkey                                      | Device-assisted access without typing a long key              | LAN origin, secure context, enrollment, and recovery                  | Consider as a separate authentication project                   |
| Email link or external SSO                   | Familiar delegated authentication                             | Adds mail or provider availability and configuration                  | Poor default for the current independent LAN happy path         |

## What belongs in Kiso

I inspected the [component gallery](https://kiso.momoi-labs.dev/#components)
and [login example](https://kiso.momoi-labs.dev/#example/login). The gallery
identifies itself as 0.17.0. It already documents FormField, helper text,
validation, alerts, and buttons with loading. Its login example is a static
email/password composition with Google sign-in, recovery, and registration.
Those actions do not match the single-Operator API key flow.

This worktree declares @momoi-labs/kiso-react ^0.11.0. The installed version
and complete exports were not checked. Confirm capabilities in the version
the console actually consumes before changing the dependency or adding a
component. I found no dedicated revealable secret component in the gallery.
That observation is not a complete source-code audit.

The proposed Kiso addition is a SecretInput or PasswordInput interaction:

- Mask the value by default and reveal it only on an explicit action.
- Allow product-specific labels such as Show API key.
- Support keyboard operation without submitting the form from the reveal button.
- Preserve the input value and integrate with FormField hints and validation.
- Pass through autocomplete and other native input attributes.
- Avoid spellcheck and automatic capitalization when the secret is visible.
- Work beside password-manager controls on narrow screens.

The final name should follow the scope chosen in Kiso. If it supports passwords
and API keys, document both and their different autocomplete needs. Do not
hard-code a credential policy in the generic component.

Reuse Kiso's existing feedback and form components. The console should own the
HTTP request, status mapping, key retrieval instructions, access persistence,
and navigation. An AuthLayout is a weaker candidate: add it only if actual
products share enough structure to justify it. A LoginPage with Google,
registration, and recovery logic would encode the wrong product assumptions.

## How to validate improvements

Compare the current screen and a revised screen using the same tasks. Ask
Operators to obtain a key, enter it, recover from a refused key, and identify a
connection failure. Observe whether they complete the task without prompting,
where they hesitate, and whether they understand the next action. With a small
qualitative sample, report observed failures rather than statistical claims.

For manual evaluation against the real backend:

1. Open /console. Tab through the field and actions. The order should follow
   the visual flow, and Enter in the field should submit once.
2. Paste a disposable test credential. The entire value should remain intact.
   In a revised screen, Show API key should reveal it and Hide API key should
   mask it without changing the value.
3. Submit an invalid test key. Expect a readable rejection and a clear next
   action, announced to assistive technology and associated with the field.
4. Test a delayed response and an unavailable Host in a controlled local
   environment. Expect visible progress, no duplicate requests, and a
   connection/service message rather than a credential rejection.
5. Test a supported password manager on the actual console origin. Verify
   fill, save behavior, and coexistence with the reveal control.
6. Repeat at a narrow viewport, browser zoom, and both themes. Help, errors,
   the reveal action, and submit should remain reachable.

The revised-screen expectations above are acceptance criteria. They have not
been implemented or tested. I inspected the available backend login visually,
but did not perform credential submissions. No automated checks ran. The
terminal tool failed because its code-mode host executable was missing, so
I could not build the console, start an updated cargo process, or run a prose
validator. The only repository change is this research document.

The next decision is whether to keep the current API key flow for the first
usability improvement. My recommendation is to keep it, verify the consumed
Kiso capabilities, and address entry, feedback, and retrieval instructions.
