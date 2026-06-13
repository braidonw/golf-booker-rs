# golf-booker

Self-hosted golf tee-time booker for a small group (family). Browse multiple
clubs, book a slot immediately, or **schedule a booking to fire the instant a
sheet opens** (e.g. 10am Friday), racing for the slot automatically.

A ground-up rewrite of the earlier `axum-booker`, on a modern stack.

## Stack

- **Web:** [axum](https://github.com/tokio-rs/axum) 0.8, [Askama](https://github.com/rinja-rs/askama) templates, [HTMX](https://htmx.org) (vendored)
- **Auth:** [axum-login](https://github.com/maxcountryman/axum-login) 0.18 + [tower-sessions](https://github.com/maxcountryman/tower-sessions) with a SQLite session store (sessions survive restarts)
- **Storage:** SQLite via [sqlx](https://github.com/launchbadge/sqlx) 0.8
- **Club client:** [reqwest](https://github.com/seanmonstar/reqwest) 0.13 (per-club cookie jar) against MiClub-style endpoints
- **Styling:** [Sugarcube](https://sugarcube.sh) design tokens + CUBE CSS, bundled to a single `assets/styles.css`
- **Deploy:** Docker on Coolify, access restricted to a Tailscale tailnet (HTTPS via a Tailscale Serve sidecar) — see [docs/DEPLOY.md](docs/DEPLOY.md)

## Development

```sh
cp .env.example .env      # then edit
just dev                  # CSS watch + cargo watch (or: cargo run)
```

The server listens on `PORT` (default 3000). The scheduler is **dry-run by
default** — set `DRY_RUN=false` to make real bookings.

### Styling

CSS is generated from the design tokens in `tokens/` and bundled into
`assets/styles.css` (committed, so the deployed app needs no Node):

```sh
just css                  # sugarcube generate + build-css.mjs
```

## Layout

```
src/
  config.rs   db.rs   error.rs   main.rs
  web/        # router, state, handlers, templates
migrations/   # SQLite schema
templates/    # Askama HTML
tokens/       # Sugarcube design tokens (source of truth for styling)
assets/       # generated CSS + vendored htmx, served at /assets
```
