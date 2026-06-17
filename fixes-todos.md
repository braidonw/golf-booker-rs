# Fixes & TODOs — code review

Findings from a review done alongside adding test coverage (2026-06-17). Nothing
here is applied yet — these are suggestions to triage together. Ordered roughly
by impact. Each is tagged with a rough severity.

The test work that prompted this added DB-backed tests for `clubs`, `users`, and
the `scheduler` job lifecycle, plus unit tests for `safe_next` and the
booking-group predicates (36 → 70 tests). The biggest *remaining* test gap is the
HTTP handler layer — see item 9.

---

## Correctness / robustness

### 1. Stale jobs fire immediately instead of being skipped — *medium*
`dispatch_due_jobs` (`src/scheduler/mod.rs:240`) selects every pending job with
`scheduled_time <= now + ARM_WINDOW`, with **no lower bound**. A job whose target
is long past (created before a multi-hour outage, say) is still "due": on the next
poll it arms, `sleep_until` returns instantly, and it fires against a sheet that
opened ages ago — pointless, and possibly booking the wrong now-open sheet.
The create handler rejects past times, but nothing guards a job that *becomes*
stale while the process is down.
**Suggest:** in `arm_and_run_job`, if `target` is more than a small grace period
in the past, fail the job ("missed scheduled time") and notify, instead of firing.

### 2. A panicking job task leaves the row stuck in `running` — *medium*
Jobs are armed via a detached `tokio::spawn(arm_and_run_job(...))`
(`src/scheduler/mod.rs:264`). If that task panics (rather than returning an
`Err`), the row stays `running` forever in a long-lived process — crash-recovery
only requeues `running` rows *at startup*, which may never come.
**Suggest:** wrap the arm body so a panic/`JoinError` marks the job failed (or add
a watchdog that requeues rows stuck in `running` past a max lifetime).

### 3. `COOKIE_SECURE` (and `DRY_RUN`) parsing is too literal — *low*
`Config::from_env` (`src/config.rs:82-89`) treats a value as false only if it is
exactly `"false"` or `"0"`. So `COOKIE_SECURE=False`, `=no`, or a stray space
silently keeps `secure = true`, and local HTTP login breaks confusingly (the
browser never returns a `Secure` cookie). For `DRY_RUN` the same quirk is
fail-safe (stays dry), but `COOKIE_SECURE` is a footgun.
**Suggest:** `.trim().eq_ignore_ascii_case("false")` / also accept `"no"`/`"off"`.

### 4. Booking success is inferred from the *absence* of an error document — *low (known)*
`interpret_booking_response` (`src/golf/client.rs:245`) treats any non-error,
non-HTML body as a successful booking, because the club's success shape isn't
pinned down. The `looks_like_login_page` guard covers the session-lapse case, but
an unexpected 200 body (maintenance JSON, a changed success format) could be
recorded as a booking that never happened. Already flagged as the "live-club smoke
test" follow-up in `docs/PLAN.md`; restating because it's the highest-stakes
assumption in the app.
**Suggest:** capture a real success body during the smoke test and assert a
*positive* success marker.

### 5. Terminal-error markers are broad substrings — *low*
`classify_booking_error` (`src/golf/client.rs:217`) flags messages containing
`"maximum"`, `"exceeded"`, `"no longer"`, etc. as terminal. A transient message
that happens to contain one of those words would stop the retry loop early during
the race. Unknown → retryable is the right default; the terminal list is the risky
part. **Suggest:** tighten against real observed messages once available.

### 6. Ambiguous local times pick the earlier instant — *very low*
`local_to_utc` (`src/web/jobs.rs:50`) maps a DST fall-back ambiguous time to the
first occurrence. Off by an hour in that one window per year; tee sheets open in
the morning, so essentially never hit. Noting for completeness.

---

## Performance / efficiency

### 7. Every club page re-authenticates from scratch — *medium*
`web::events` builds a fresh `GolfClient` (new cookie jar) and calls `.login()` on
every list/detail/schedule/book request (`src/web/events.rs:208,254,328` and
`fetch_events`). Browsing thus hammers the club with a login round-trip per page
view and never reuses a session. **Suggest:** cache an authenticated client per
club (with re-login on 401/expiry), or at least reuse one client across the calls
within a single request.

### 8. `get_event_meta` fetches the full 60-day events list to find one event — *low*
`get_event_meta` (`src/golf/client.rs:125`) calls `get_events()` (a 60-day list)
just to `find` one id. The detail and schedule pages call it *in addition to*
`get_event`, so each page makes a large redundant fetch. **Suggest:** request a
narrow date window, or thread the already-known event metadata through.

---

## Tests / maintainability

### 9. No HTTP-handler integration tests — *medium (test gap)*
Handler logic (login gating/redirects, form validation, the
`/scheduled-jobs` create flow, HTMX vs full-page branches) is only exercised
indirectly. The pure helpers and DB functions are now covered, but the wiring is
not. **Suggest:** add axum integration tests via `tower::ServiceExt::oneshot`
against a router built over an in-memory DB (the new `test_support::test_pool`
helper is a starting point), seeding a session for the authenticated routes.

### 10. Reset tokens are never garbage-collected — *low*
`password_reset_tokens` rows are inserted but never deleted; used/expired tokens
accumulate forever (`src/users.rs`). Harmless at family scale, but unbounded.
**Suggest:** delete a user's prior tokens when issuing a new one, and/or a periodic
sweep of expired/used rows.

### 11. `schedule_booking` trusts the posted slot without re-checking it — *low*
The `/scheduled-jobs` POST (`src/web/jobs.rs` `create`) validates club + time but
not that the event/group still exists or has a free member seat. A stale page
could schedule a job that's guaranteed to fail at fire time. The fire path handles
it, so this is just earlier/friendlier feedback. **Suggest (optional):** a
best-effort `find_group` + `is_schedulable` check at creation.

---

## Non-issues confirmed during review (no action)

- Manual `Debug` impls correctly redact secrets on `Club`, `User`, `GolfClient`,
  `SmtpConfig` (now covered by tests).
- `claim_job` is atomic (pending→running) — verified no double-arm under the
  test; the dispatcher's claim-then-spawn is sound.
- Constant-time auth path verifies against a dummy hash when the user is absent
  (`src/users.rs`) — no username-enumeration timing leak.
- `safe_next` blocks protocol-relative (`//`, `/\`) and absolute open redirects.
- The `updated_at` trigger doesn't recurse (SQLite recursive triggers are off by
  default).
- Plaintext club credentials are an accepted, documented trade-off (single
  operator; must be replayed to the club). Encrypt-at-rest already a tracked
  follow-up in `docs/PLAN.md`.
