# Style & review guide

Conventions for `golf-booker`. The goal is code that reads like one person wrote
it. When in doubt, match the surrounding code over any rule here.

These double as a **review checklist**: a reviewer (human or Claude) should check
changed code against the relevant sections below.

---

## Principles

- **Clarity over cleverness.** Optimise for the next reader.
- **Comments explain *why*, not *what*.** The code already says what. Reserve
  comments for intent, trade-offs, and non-obvious constraints (e.g. why login
  happens 30s before the firing moment).
- **Errors are surfaced, never swallowed.** No empty `catch`/`Err(_) => {}` that
  hides a failure. If a failure is genuinely ignorable, say why in a comment.
- **Secrets never leave the process.** Not in logs, not in templates, not in
  error messages.

---

## Rust

### Errors

- Handlers return `Result<_, AppError>` and use `?`. Never `unwrap`/`expect`/
  `panic!` on a path reachable from a request. `panic!`/`expect` are acceptable
  only for genuine startup invariants in `main`/`App::new`.
- Use `anyhow` for application errors; reserve `thiserror` for typed errors that
  callers branch on.
- Map to the right status: default `AppError` is 500; use `AppError::not_found`
  for 404. Don't return 200 with an error body.

### Secrets

- Any struct holding credentials (`User`, `Club`, `GolfClient`) implements
  `Debug` **manually** with secret fields rendered as `"[redacted]"`. If you add
  a secret-bearing field, update its `Debug`.
- Never interpolate a password/token/member id into a log line or template.
- Club credentials are plaintext at rest by necessity — don't widen their
  exposure (e.g. don't echo them back in a form or API response).

### Async & performance

- CPU-bound work (argon2 hashing/verification) runs on `tokio::task::
  spawn_blocking`, never inline in a handler.
- Don't hold a lock across an `.await` you don't have to.
- The booking hot path is latency-sensitive: keep per-attempt work minimal and
  do setup (login) ahead of the firing moment.

### Database (sqlx)

- Use **runtime** queries: `sqlx::query`/`query_as` with `.bind()`, never the
  compile-time `query!` macros (keeps the build DB-free). Always bind
  parameters — never format values into SQL.
- Map rows with `#[derive(FromRow)]`. Timestamps are stored as RFC3339 `TEXT`
  and parsed with chrono; store UTC, convert to a club's local zone for display.
- Schema changes go in a new `migrations/NNNN_*.sql` (never edit a shipped one).

### Logging

- `tracing` with structured fields: `tracing::info!(job_id, %club.name, "armed")`,
  not string concatenation. Pick levels deliberately: `error!` for failures
  needing attention, `warn!` for recoverable/odd, `info!` for lifecycle, `debug!`
  for detail. No `dbg!`/`println!` in committed code.

### Modules & naming

- Handler files expose a `router(state)` and split methods into inner
  `mod get` / `mod post` blocks, matching `web/auth.rs` and `web/protected.rs`.
- Domain logic (DB access, types) lives in crate-root modules (`users.rs`,
  `clubs.rs`, `scheduler/`, `golf/`); HTTP/handlers live under `web/`.
- `snake_case` items, `CamelCase` types, `SCREAMING_SNAKE_CASE` consts. Keep
  tuning knobs as named `const`s with a doc comment (see `scheduler`).

### Hygiene

- Code must compile with no new warnings. Don't leave `#[allow(dead_code)]` to
  silence genuinely unused code — delete it or wire it up. Run `cargo fmt`.

---

## HTML / Askama

- Every page `{% extends "base.html" %}`; render via `web::render(&template)`.
- **Semantic HTML first.** Use the right element (`<button>`, `<nav>`, `<table>`,
  `<label>` wrapping its input) — `global.css` styles these from tokens, so
  correct markup is most of the styling.
- Askama auto-escapes interpolations. For strings you build by hand in Rust, use
  `html_escape::encode_text`. Never build HTML by string-concatenating user or
  club data unescaped.
- Accessibility: label every input; use `aria-*`/`role` where semantics need
  reinforcing (e.g. `role="alert"` on error banners); keep focus order sane.
- **HTMX:** prefer progressive enhancement — forms work without JS, HTMX makes
  them snappier. Return small partial templates (under `templates/partials/`)
  from `hx-`targeted endpoints; don't return a whole page into a fragment.
  Mutations use `hx-post`/`hx-delete`, not `hx-get`.

---

## CSS (Sugarcube / CUBE CSS)

- **No utility-class soup.** This isn't Tailwind. Reach, in order:
  1. correct semantic element (styled globally),
  2. a **composition** for layout (`.wrapper`, `.flow`, `.cluster`, `.repel`,
     `.grid`, `.switcher`, `.sidebar`),
  3. a **component** class (`.button`, `.card`, `.input`, `.alert`, `.badge`),
  4. only then a one-off.
- **Use design tokens, never magic numbers.** Spacing/colour/type come from
  `var(--space-*)`, `var(--color-*)`, `var(--text-*)`. A hardcoded `1rem`/hex is a
  smell — use or add a token.
- **Systemic change → edit `tokens/`** and run `just css`. **One-off override →**
  add it to `assets/css/app.css` (the exceptions layer). Never hand-edit the
  generated `assets/css/tokens.css`, `utilities.css`, `cube/*`, `components/*`,
  or `assets/styles.css` — they're regenerated.
- Compositions accept per-instance config via their custom properties (e.g.
  `style="--cluster-gap: var(--space-sm)"`) — prefer that over new classes.
- Commit the regenerated `assets/styles.css` alongside token changes.

---

## Review checklist (quick pass)

- [ ] No secret in a log, template, or error message; secret structs still
      redact in `Debug`.
- [ ] Errors handled and surfaced; no silent `Err(_)` swallowing.
- [ ] Request paths use `?`/`AppError`, no `unwrap`/`expect`/`panic`.
- [ ] SQL uses bound parameters; new schema is a new migration.
- [ ] Input from the user/URL is validated (remember the `next` open-redirect):
      treat redirects, IDs, and external data as untrusted.
- [ ] `DRY_RUN` is honoured on any path that could make a real booking.
- [ ] Booking/scheduling times are timezone-correct (UTC stored, club-local
      shown).
- [ ] Markup is semantic + accessible; styling uses tokens/compositions, not
      magic numbers; `app.css` for overrides.
- [ ] Builds with no new warnings; `cargo fmt` clean; logic has a test where
      practical (especially scheduler timing and parsing).
