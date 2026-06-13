# Deploying on Coolify behind Tailscale

The goal: run golf-booker on a Coolify host, reachable **only** by family members
on your Tailscale tailnet, over HTTPS — never exposed to the public internet.

How it works: a **Tailscale sidecar** container joins your tailnet and runs
`tailscale serve`, terminating HTTPS and proxying to the app on `127.0.0.1:8080`.
The app container shares the sidecar's network namespace (`network_mode:
service:tailscale`), so it has no public ports at all. The result is a stable URL
like `https://golf-booker.<your-tailnet>.ts.net`.

## Prerequisites

- A Coolify instance (any server Coolify manages).
- A Tailscale account + tailnet, with **MagicDNS** and **HTTPS** enabled
  (Admin console → DNS → enable MagicDNS and "HTTPS Certificates").
- The family members you want to grant access added to the tailnet (or use
  node sharing — see "Granting access").

## 1. Create a Tailscale auth key

Tailscale admin console → **Settings → Keys → Generate auth key**:

- **Reusable**: on (so redeploys can re-auth).
- **Ephemeral**: optional — if on, the node disappears when the container stops.
  Prefer **off** here so the node identity is stable across restarts (the
  `tailscale-state` volume also preserves it).
- **Tags**: optionally tag it (e.g. `tag:golf`) and gate access with an ACL.

Copy the key (`tskey-auth-…`). Treat it as a secret.

## 2. Deploy on Coolify

Create a new **Docker Compose** resource pointing at this repo (it contains
`docker-compose.yml`, the `Dockerfile`, and `tailscale/serve.json`).

Set these environment variables (Coolify → the resource → Environment Variables;
mark the secrets as such):

| Variable | Required | Notes |
|---|---|---|
| `TS_AUTHKEY` | ✅ | the `tskey-auth-…` from step 1 |
| `APP_USERNAME` | ✅ (first boot) | your login username |
| `APP_PASSWORD` | ✅ (first boot) | your login password — seeds the first account on an empty DB |
| `DRY_RUN` | – | defaults `true` (simulates bookings); set `false` to book for real |
| `COOKIE_SECURE` | – | defaults `true`; leave on (Tailscale Serve gives HTTPS) |
| `RIDGE_*` / `NSW_*` | – | optional club seeding (see `.env.example`); clubs can also be added in the UI |

Two named volumes are declared and persist across redeploys:

- `golf-data` → `/data` — the SQLite DB (your account, clubs, scheduled jobs).
- `tailscale-state` → the node's identity/keys.

> The `golf-data` volume holds club credentials (plaintext, since they must be
> replayed to the club). Protect the host and back this volume up.

Deploy. On first boot the app runs migrations and seeds the account + any clubs
from the env vars (you'll see `seeded initial login account from environment` in
the logs).

## 3. Access it

Open `https://golf-booker.<your-tailnet>.ts.net` from any device on the tailnet
and sign in. (The hostname is the `hostname:` set in `docker-compose.yml` — change
it there if you want a different name.)

### Granting access to family

- Add them to your tailnet (invite from the admin console), **or**
- Share just this node with their tailnet (Machines → the `golf-booker` node →
  Share), **or**
- Restrict access with an ACL targeting the node's tag, e.g.:

  ```jsonc
  // Tailscale ACLs
  "acls": [
    { "action": "accept", "src": ["group:family"], "dst": ["tag:golf:443"] }
  ]
  ```

## 4. Going live (real bookings)

The scheduler and "Book now" both simulate while `DRY_RUN=true`. When you've
confirmed everything works against your real club:

1. Add/verify your clubs at `/clubs` (credentials + correct IANA timezone).
2. Set `DRY_RUN=false` in Coolify and redeploy.
3. Do one real "Book now" on a low-stakes slot to confirm the live login/book
   path before relying on a scheduled job.

## Updating

Push changes and redeploy in Coolify. The `golf-data` volume persists, so your
account, clubs, and scheduled jobs survive the redeploy.

## Alternative: no sidecar

If your Coolify host is itself already on the tailnet, you can instead deploy
just the `Dockerfile` (no compose), publish the port only on the host's Tailscale
interface, and reach it at `http://<host-tailscale-ip>:8080`. In that case there
is no HTTPS, so set `COOKIE_SECURE=false`. The sidecar approach above is
preferred because it gives a clean hostname and real HTTPS.

## Troubleshooting

- **Can't reach the URL**: check the `tailscale` container logs; confirm the node
  appears in the admin console and MagicDNS + HTTPS certs are enabled.
- **Login bounces back to /login**: `COOKIE_SECURE=true` over plain HTTP drops the
  cookie. Use the HTTPS `ts.net` URL (not an IP), or set `COOKIE_SECURE=false`.
- **DB resets on redeploy**: the `golf-data` volume isn't persisting — check the
  Coolify volume mapping to `/data`.
