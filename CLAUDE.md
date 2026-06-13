# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`golf-booker` is a self-hosted web app for a family to book golf tee times across
multiple clubs. Its defining feature is **scheduled bookings**: club tee sheets
open at a fixed local time (e.g. 10am Friday) and everyone races to book — this
app schedules a job that fires at that instant and books automatically.

It is a ground-up rewrite of the sibling `../axum-booker` (kept as reference
only — do not edit it). The rewrite is being built in numbered phases; see
`docs/PLAN.md` for the phase list and current progress.

## Commands

```sh
cargo run                 # run the server (defaults: port 3000, dry-run on)
cargo build               # build
cargo check               # fast type-check
cargo test                # run tests
cargo test <name>         # run a single test by substring match
just dev                  # CSS watch + cargo-watch together (preferred dev loop)
just css                  # regenerate CSS after editing tokens/ (see Styling)
```

There is no DATABASE_URL needed at build time: queries are **runtime-checked**
(`sqlx::query`/`query_as`), not the compile-time `query!` macros, so the build
never touches a database and no `.sqlx` offline cache exists. Keep it that way
unless you deliberately switch the whole crate over.

## Environment

Copy `.env.example` to `.env` (loaded automatically in dev via `dotenvy`).

- `DRY_RUN` — **defaults to true**; the scheduler logs instead of booking.
  Real bookings only happen with `DRY_RUN=false`.
- `COOKIE_SECURE` — defaults to true (deployed behind TLS). Set `false` for
  local plain-HTTP dev or the browser never returns the session cookie.
- `APP_USERNAME` / `APP_PASSWORD` — seed the first login account on an empty DB.
- `PORT`, `DATABASE_URL` (default `sqlite:golf.db`).

## Architecture

Request flow: `main.rs` → `web::App::new()` (config, DB+migrations, user seeding)
→ `App::serve()` builds the router and runs `axum::serve`.

**Two kinds of "login" — keep them distinct.**
- *App accounts* (`users` table, `src/users.rs`): the family members who sign in.
  Auth is axum-login 0.18 + tower-sessions with a **SQLite session store**, so
  sessions survive restarts. Passwords are argon2 (`password-auth`), verified on
  a blocking thread; the user-absent path still verifies against a dummy hash to
  avoid timing-based username enumeration. Accounts are managed at `/users`;
  passwords reset via emailed single-use tokens (`/forgot`, `/reset`).
- *Club logins* (`clubs` table, `src/clubs.rs`): credentials for each golf club,
  stored centrally (single operator). Stored **plaintext** because they must be
  replayed to the club on login — treat the whole row as a secret. Both `User`
  and `Club` implement `Debug` manually to redact secrets from logs.

**Routing/auth layering** (`src/web/app.rs`): protected routers are merged, then
gated with `login_required!(Backend, login_url = "/login")`; the auth router
(`/login`, `/logout`) is merged outside the gate; `/health` and `/assets` are
public. The whole tree is wrapped in the `AuthManagerLayer`. axum-login 0.18's
layer is infallible — do **not** wrap it in `HandleErrorLayer` (it breaks the
`Service` bound).

**Per-club HTTP client** (`src/golf/`, added in Phase 3): one `GolfClient` per
club, each with its own cookie jar, built from a `Club` row via
`GolfClient::from_club`. Talks to MiClub-style endpoints (Spring URLs; responses
are a mix of JSON and XML parsed with serde / quick-xml).

**Email** (`src/email.rs`): a `Mailer` over lettre (SMTP, Fastmail by default,
rustls). Optional — with no SMTP config it's disabled and callers fall back
(reset links get logged, notifications skipped). Sends password-reset links and
booking-outcome notifications. Links use `config.base_url` (`APP_BASE_URL`).

**Scheduler** (`src/scheduler/`, added in Phase 4) is the heart of the app. A
dispatcher polls for jobs whose firing time is near, atomically claims each
(`pending`→`running`), and hands it to a dedicated task that: pre-authenticates
~30s early (fresh cookies), sleeps until the exact moment, then fires the
booking POST with rapid retries for a few seconds. Crash recovery re-queues
`running` jobs on startup. Times are stored UTC but **interpreted/displayed in
each club's IANA `timezone`** because sheets open at local time. When porting
logic from `../axum-booker`, carry the firing design but fix its bugs (it treated
`datetime-local` input as UTC, and only ever browsed the first club).

**Templates** (`src/web/`, `templates/`): Askama, rendered via the
`web::render(&template)` helper (askama 0.13+ dropped the bundled axum
integration). HTMX is **vendored** at `assets/vendor/` — no CDN. `AppError`
(`src/error.rs`) lets handlers use `?`; anything `Into<anyhow::Error>` becomes a
500, with `AppError::not_found` for 404s.

## Styling (Sugarcube + CUBE CSS)

Styling is **not** Tailwind. Design tokens live in `tokens/` (DTCG JSON, the
source of truth). The Sugarcube CLI generates CSS custom properties, and CUBE
CSS provides semantic element styling + layout compositions + components.

- Pipeline: edit `tokens/` → `just css` (runs `sugarcube generate` then
  `build-css.mjs`) → bundles everything into `assets/styles.css` wrapped in
  `@layer tokens, global, compositions, utilities, components, exceptions`.
- `assets/css/app.css` is the hand-written **exceptions** layer; it survives
  regeneration. Put app-specific overrides there, not in generated files.
- `assets/styles.css` and the generated `assets/css/*` are **committed** so the
  deployed app needs no Node at runtime.
- Pinned to `@sugarcube-sh/cli@0.1.16` (a very new tool — its API may shift on
  upgrade). See `docs/STYLE.md` for how to write markup against this system.

## Conventions

- Follow `docs/STYLE.md` for Rust / HTML / CSS style and review expectations.
- Match the surrounding code: handler modules use inner `mod get`/`mod post`
  blocks; comments explain *why*, not *what*.
- The dev `golf.db*` files are gitignored; never commit them or `.env`.
