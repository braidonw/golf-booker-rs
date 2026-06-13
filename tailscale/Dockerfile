# Tailscale sidecar with the serve config baked in, so it never depends on a
# host bind mount (which Coolify's deploy context turned into a directory,
# breaking `tailscale serve`).
FROM tailscale/tailscale:latest
COPY tailscale/serve.json /config/serve.json
