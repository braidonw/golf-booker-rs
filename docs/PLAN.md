# Build plan & progress

A fresh rewrite of `../axum-booker` (untouched reference) on a modern stack,
carrying over the good parts — chiefly the scheduler's firing design and the
golf-client logic — while fixing its rough edges (plain-HTML pages, hardcoded
dates, single-club browsing, UTC/local timezone confusion) and replacing the
fly.io deploy with Coolify + Tailscale.

## Stack

axum 0.8 · axum-login 0.18 · tower-sessions 0.14 (+ SQLite store) · askama 0.16 ·
sqlx 0.8 (SQLite) · reqwest 0.13 · chrono + chrono-tz · Sugarcube/CUBE CSS · HTMX.

Version constraints worth remembering: axum-login 0.18 pins tower-sessions 0.14;
the sqlx session store pins sqlx 0.8. Don't bump one in isolation.

## Phases

- [x] **0 — Scaffold.** Crate, config, DB pool (WAL) + migrations, `AppError`,
      base layout, Sugarcube/CUBE CSS pipeline, vendored HTMX, home + health.
- [x] **1 — Auth + persistent sessions.** axum-login backend, SQLite session
      store, login/logout, `login_required`, env-seeded first account.
      Hardened after security review (open-redirect, CSRF logout, cookie flags,
      constant-time auth).
- [x] **2 — Clubs.** `clubs` model + CRUD pages (Askama/HTMX), per-club IANA
      timezone, env-seeding for migration, credentials never rendered back.
- [x] **3 — Golf client + browsing.** Port/clean `GolfClient` (login, get_events,
      get_event, book; drop hardcoded dates/`dbg!`). Event browsing with a club
      selector; view a booking group; book-now. Carry the chosen club through.
- [x] **4 — Scheduler + jobs UI.** Port the arm → pre-auth → sleep-to-moment →
      rapid-retry scheduler, timezone-aware (UTC stored, club-local in/out).
      Jobs list/create/cancel; "schedule from slot" prefill. Dry-run default.
- [x] **5 — Deploy.** Multi-stage Dockerfile (verified building + running
      locally), persistent volume for SQLite, secrets via env, docker-compose
      with a Tailscale Serve sidecar for HTTPS, tailnet-only access. See
      `docs/DEPLOY.md`.
- [x] **6 — Polish.** Login rate-limiting (per-username, in-memory), booking
      error classification (terminal failures stop the retry loop), scheduler
      timing tests, warning-free build.
- [x] **7 — Users & email.** User management UI (`/users`), email-based password
      reset (single-use hashed tokens), and booking-outcome notification emails,
      all over Fastmail SMTP (lettre) with graceful disable when unconfigured.

## Deferred / follow-ups

- **Live-club smoke test**: the login/book POST and event/sheet parsing are only
  exercised against a mock. Verify against the real club (add it, browse, then
  one real `DRY_RUN=false` booking) before relying on a scheduled job.
- **Live email send**: the reset/notification *content and fallbacks* are tested,
  but actual SMTP delivery needs real Fastmail credentials — send one test.
- Encrypt club credentials at rest (currently plaintext by necessity).
- Terminal booking errors still consume `max_attempts` re-arms before failing
  (the retry-loop classification works; cross-attempt short-circuit is a TODO).
- Per-IP rate limiting only becomes meaningful if exposed beyond the tailnet
  (behind the Tailscale proxy every request shares one source IP).
