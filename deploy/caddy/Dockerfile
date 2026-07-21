# Caddy with the Cloudflare DNS provider compiled in — required for the
# DNS-01 wildcard certificate on *.play.<domain> (M5). Harmless before that.
FROM caddy:2-builder AS builder
RUN xcaddy build --with github.com/caddy-dns/cloudflare

FROM caddy:2
COPY --from=builder /usr/bin/caddy /usr/bin/caddy
