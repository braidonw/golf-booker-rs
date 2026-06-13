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
- [ ] **2 — Clubs.** `clubs` model + CRUD pages (Askama/HTMX), per-club IANA
      timezone, env-seeding for migration, credentials never rendered back.
- [ ] **3 — Golf client + browsing.** Port/clean `GolfClient` (login, get_events,
      get_event, book; drop hardcoded dates/`dbg!`). Event browsing with a club
      selector; view a booking group; book-now. Carry the chosen club through.
- [ ] **4 — Scheduler + jobs UI.** Port the arm → pre-auth → sleep-to-moment →
      rapid-retry scheduler, timezone-aware (UTC stored, club-local in/out).
      Jobs list/create/cancel; "schedule from slot" prefill. Dry-run default.
- [ ] **5 — Deploy.** Multi-stage Dockerfile, persistent volume for SQLite,
      secrets via env, Coolify config, Tailscale tailnet-only access.
- [ ] **6 — Polish.** Scheduler timing tests, booking error classification
      (retryable vs terminal), login rate-limiting (deferred from Phase 1),
      cleanup.

## Deferred / follow-ups

- Login rate-limiting (per IP + per username) before any non-tailnet exposure.
- Encrypt club credentials at rest (currently plaintext by necessity).
- Booking error classification to retry only retryable failures.
