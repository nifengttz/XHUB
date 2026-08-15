# XHUB Wallet V3.6 single-VPS deployment

This profile runs the public Wallet UI on `127.0.0.1:18736` behind Caddy and
connects to the authenticated HUB on `127.0.0.1:18737` through a fixed-route
server-side gateway.

The HUB bearer token is mounted read-only at
`/run/secrets/hub-api-token.txt`. It is never returned by the Wallet config
endpoint or embedded in browser assets. The gateway accepts only HUB health,
signed reservation submission, and original-nonce status lookup routes. Its
upstream URL must resolve to a loopback host.

The deployment remains a mainnet canary with `production_ready=false` and
`production_broadcast=false`. It does not create or broadcast transactions.
