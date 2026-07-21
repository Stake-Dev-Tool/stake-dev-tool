# Deploying the cloud server

One box runs everything: Postgres + the server binary (API + dashboard +
LGS) + Caddy (TLS). Identical for the hosted instance and self-hosters —
that is the point.

## Prerequisites

- A Linux server (8 GB RAM recommended) with Docker + the compose plugin.
- DNS at Cloudflare (or anywhere for the app domain):
  - `A app.<domain>` → the server's IP — **DNS only** (grey cloud).
  - M5+: `A *.play.<domain>` → same IP — **DNS only**, plus a Cloudflare
    API token (Zone → DNS → Edit) for the wildcard certificate.
- Ports 80 and 443 open (and 22 for you). Nothing else.

## First deploy

```bash
git clone https://github.com/simnJS/stake-dev-tool.git && cd stake-dev-tool/deploy
cp .env.prod.example .env.prod && $EDITOR .env.prod   # domains, DB password, R2 keys
GIT_SHA=$(git rev-parse --short HEAD) docker compose -f docker-compose.prod.yml --env-file .env.prod up -d --build
curl -s https://app.<domain>/healthz                  # {"status":"ok",...}
```

The server runs migrations itself on startup (retrying until Postgres is
up), so there is no separate migration step.

## Updates

```bash
git pull
GIT_SHA=$(git rev-parse --short HEAD) docker compose -f docker-compose.prod.yml --env-file .env.prod up -d --build
```

Zero-ceremony: the binary is stateless (state lives in Postgres + the
object store), so a restart is a deploy. Take a provider snapshot before
pulling a release that includes migrations if you want a rollback path.

## Backups

Postgres is the critical state (with `STORAGE_BACKEND=fs`, the `blobdata`
volume too). Minimum viable cron (daily, keep 14):

```bash
docker compose -f docker-compose.prod.yml exec -T postgres \
  pg_dump -U stakedev stakedev | zstd > backup-$(date +%F).sql.zst
```

Ship those files off the box (e.g. `rclone` to an R2 bucket). Provider
snapshots (Hetzner/Netcup) on top are cheap insurance.

## Notes

- The dashboard is served by the server itself (`SERVER_WEB_DIR=/app/web`,
  baked into the image) — no separate frontend deployment exists.
- `SERVER_COOKIE_SECURE=true` is required behind TLS, and
  `SERVER_PUBLIC_URL` drives invite/device links — set both.
- The wildcard `*.play` block in `Caddyfile` stays commented until share
  v2 (M5) ships.
