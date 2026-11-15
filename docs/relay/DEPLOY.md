# Deploying hame-relay

Status: built and tested locally. **Nothing is deployed without the
owner's go-ahead.** This guide describes a single VPS with Docker, the
relay behind Caddy for TLS.

## 1. What runs

- `hame-relay`: one process, one SQLite database file (WAL mode) in
  `/data`. It listens on plain HTTP inside the Docker network.
- Caddy: terminates TLS (automatic certificates), proxies HTTP and
  WebSocket to the relay, overwrites `X-Forwarded-For` with the client
  address.

The relay keeps live WebSocket connections and the rate-limit buckets in
memory, so run **one** relay process. Scaling out needs a shared
database (PostgreSQL behind the `Repository` trait) and a pub/sub for
wake-ups; neither exists yet.

## 2. Build the image

From the repository root:

```sh
docker build -f crates/hame-relay/Dockerfile -t hame-relay .
```

The runtime image is `gcr.io/distroless/cc-debian12:nonroot`: no shell,
user 65532, under 70 MB. `hame-relay healthcheck` is the container
health check.

## 3. Configure

Copy the examples and edit them:

```sh
mkdir -p deploy/secrets
cp docs/relay/docker-compose.example.yml deploy/docker-compose.yml
cp docs/relay/Caddyfile.example deploy/Caddyfile
cp docs/relay/relay.env.example deploy/relay.env
# secrets, one value per file, mode 0600
printf '%s' 'smtp password' > deploy/secrets/smtp_password
printf '%s' 'google client secret' > deploy/secrets/google_client_secret
cp AuthKey_KEY1234567.p8 deploy/secrets/apple_key.p8
chmod 600 deploy/secrets/*
cd deploy && docker compose up -d --build
```

Every variable that holds a secret can instead name a file with the
`_FILE` suffix (`RELAY_SMTP_PASSWORD_FILE=/run/secrets/smtp_password`).

### Environment variables

| Variable | Default | Meaning |
|---|---|---|
| `RELAY_PUBLIC_URL` | (required) | Public base URL, e.g. `https://relay.example.com`. OAuth redirect URIs are derived from it. |
| `RELAY_APP_URL` | public URL | Web app base URL for email links (`/verify`, `/reset-password`) and the device approval page (`/device`). |
| `RELAY_BIND` | `127.0.0.1:8080` (image: `0.0.0.0:8080`) | Listen address. |
| `RELAY_DATABASE_PATH` | `relay.db` (image: `/data/relay.db`) | SQLite file. |
| `RELAY_CORS_ORIGINS` | empty | Comma-separated origins allowed to call the API from a browser. Empty: none. |
| `RELAY_TRUST_PROXY` | `false` | Use the last `X-Forwarded-For` entry as the client IP. Set only behind a proxy that overwrites it, and read "Trusting the proxy" below. |
| `RELAY_HSTS` | `true` for https | Send `Strict-Transport-Security`. |
| `RELAY_MAX_BODY_BYTES` | `65536` | Largest HTTP request body. |
| `RELAY_LOG` | `info` | Log filter (`tracing` syntax, e.g. `info,hame_relay=debug`). |
| `RELAY_LOG_FORMAT` | `json` | `json` or `text`. |
| `RELAY_MAILER` | (required) | `smtp`, `log` (development: mails go to the log, including their reset links) or `disabled`. `log` with an https `RELAY_PUBLIC_URL` is refused unless `RELAY_DEV=1`. |
| `RELAY_DEV` | `false` | Development mode: allows the `log` mailer with an https public URL. Never on a public relay. |
| `RELAY_SMTP_HOST`, `RELAY_SMTP_PORT` | –, `587` | SMTP server. |
| `RELAY_SMTP_TLS` | `starttls` | `starttls`, `tls` (implicit, usually port 465) or `none` (local relay only). |
| `RELAY_SMTP_USERNAME`, `RELAY_SMTP_PASSWORD(_FILE)` | – | SMTP credentials. |
| `RELAY_MAIL_FROM` | (required for smtp) | `hame <no-reply@example.com>` |
| `RELAY_GOOGLE_CLIENT_ID`, `RELAY_GOOGLE_CLIENT_SECRET(_FILE)` | – | Enables Google sign-in. |
| `RELAY_GOOGLE_ISSUER` | `https://accounts.google.com` | For testing only. |
| `RELAY_APPLE_CLIENT_ID` | – | Apple Services ID; enables Apple sign-in. |
| `RELAY_APPLE_TEAM_ID`, `RELAY_APPLE_KEY_ID`, `RELAY_APPLE_PRIVATE_KEY(_FILE)` | – | The Sign in with Apple key (`.p8`) that signs the client secret. |
| `RELAY_APPLE_ISSUER` | `https://appleid.apple.com` | For testing only. |
| `RELAY_OAUTH_RETURN_URLS` | empty | Exact URLs the OAuth callback may redirect to with a one-time code (e.g. `https://app.example.com/oauth,hame://oauth`). |
| `RELAY_WEBAUTHN_RP_ID` | – | Enables passkeys; the registrable domain (e.g. `example.com`). |
| `RELAY_WEBAUTHN_RP_NAME` | `hame` | Shown by authenticators. |
| `RELAY_WEBAUTHN_ORIGINS` | (required with RP ID) | Comma-separated origins that may use passkeys (web app; Android `android:apk-key-hash:…`). |
| `RELAY_APNS_TOPIC`, `RELAY_APNS_TEAM_ID`, `RELAY_APNS_KEY_ID`, `RELAY_APNS_PRIVATE_KEY(_FILE)` | – | Enables APNs (token auth). |
| `RELAY_APNS_SANDBOX` | `false` | Use the APNs sandbox. |
| `RELAY_FCM_SERVICE_ACCOUNT(_FILE)` | – | Firebase service-account JSON; enables FCM HTTP v1. |
| `RELAY_RATE_AUTH_IP_PER_MIN` | `30` | Requests per minute per IP on `/v1/auth/*`. |
| `RELAY_RATE_AUTH_ACCOUNT_PER_MIN` | `10` | Attempts per minute per account or email (login, sign-up, reset, passkey, device-code approval). |
| `RELAY_RATE_WS_FRAMES_PER_SEC` | `50` | WebSocket frames per second per connection (burst ×4). |
| `RELAY_ARGON2_CONCURRENCY` | `4` | Password hashes at once (64 MiB each); a request that waits more than 5 s for a slot gets `503 busy`. |
| `RELAY_RATE_GLOBAL_FAILED_LOGINS_PER_MIN` | `300` | Failed password logins per minute across all addresses; beyond it password logins are refused for a while. |
| `RELAY_MAX_DEVICES_PER_ACCOUNT` | `20` | Devices (not revoked) per account. |
| `RELAY_QUEUE_MAX_MB_PER_DEVICE` | `64` | Queued envelope bytes per device. |
| `RELAY_QUEUE_MAX_MB_PER_ACCOUNT` | `256` | Queued envelope bytes per account. |
| `RELAY_MIN_FREE_DISK_MB` | `1024` | New envelopes are refused while the database's file system has less free space (Unix only). |
| `RELAY_MAX_WS_PER_ACCOUNT` | `20` | Open WebSocket connections per account. |
| `RELAY_ENUMERATION_KEY(_FILE)` | random per start | 32 bytes base64url; keeps fake passkey ids for unknown emails stable across restarts. |

Fixed by design (code constants or defaults): access token 15 min,
refresh token 30 days, queue retention 24 h, 10 000 queued envelopes per
device, envelope limit 1 MiB, delivery batches of 4 MiB, recent
authentication window for adding passkeys 5 min, 100 000 rate-limit
buckets, device codes 10 min with a 5 s interval,
ping every 25 s, idle close after 75 s.

### Trusting the proxy

With `RELAY_TRUST_PROXY=true` the relay believes the last
`X-Forwarded-For` entry. That is right only when **every** request
passes through a proxy that sets that entry itself:

- **Never publish the relay port** (`ports:` on the relay service, or a
  host firewall that lets 8080 through). A client that reaches the relay
  directly can send any `X-Forwarded-For` and pick its own rate-limit
  key. The example compose file only `expose`s the port to Caddy.
- The proxy must **overwrite** the header (the example Caddyfile sets it
  to `{remote_host}`), not append to a client-supplied one; with an
  appending proxy the last entry is still the proxy's own view, which is
  fine, but check it.
- **Behind a CDN** (Cloudflare and the like) the address Caddy sees is
  the CDN's edge, so every client shares a few rate-limit buckets.
  Either configure Caddy's `trusted_proxies` with the CDN's ranges and
  take the client address from the CDN's header (`CF-Connecting-IP`),
  or leave `RELAY_TRUST_PROXY` off and accept per-edge limits.
- Without a proxy (or with one that does not set the header) leave it
  `false`: the socket peer address is then used.

IPv6 clients are limited per /64.

## 4. Identity providers

**Google** (Google Cloud console → APIs & Services → Credentials → OAuth
client ID, type "Web application"): authorized redirect URI
`https://relay.example.com/v1/auth/oauth/google/callback`.

**Apple** (developer account → Certificates, Identifiers & Profiles):

1. An App ID with "Sign in with Apple".
2. A Services ID (this is `RELAY_APPLE_CLIENT_ID`) with the domain
   `relay.example.com` and return URL
   `https://relay.example.com/v1/auth/oauth/apple/callback`.
3. A key with "Sign in with Apple" → download the `.p8` once
   (`RELAY_APPLE_PRIVATE_KEY_FILE`), note its key id and the team id.

Apple returns with a cross-site form POST; the flow cookie is therefore
`SameSite=None; Secure`, which needs the public URL on https.

**Passkeys**: `RELAY_WEBAUTHN_RP_ID` must be the web app's domain or a
parent of it; the iOS app needs the associated domain
`webcredentials:example.com`, the Android app the Digital Asset Links
file on that domain.

**Push**: APNs uses a token key (`.p8`) with the app's bundle id as topic;
FCM a service account with the Firebase Messaging role.

## 5. Operations

- **Backups**: the database is `relay.db` plus `relay.db-wal`. Use
  SQLite's online backup rather than copying files, e.g. from a sidecar
  with the `sqlite3` CLI on the volume:
  `sqlite3 /data/relay.db ".backup '/backup/relay-$(date +%F).db'"`.
  Backups contain account data (names, emails, password hashes, public
  keys, queued ciphertext): encrypt them and restrict access.
- **Upgrades**: migrations run at start; take a backup first. Rolling
  back the binary after a migration needs the backup.
- **Logs**: JSON on stdout. They contain route templates, statuses,
  latencies, account and device ids and audit events; never tokens,
  passwords, query strings or envelope contents. Keep the proxy's access
  log free of query strings and auth headers (the example Caddyfile
  filters them).
- **Audit log**: table `audit_log` (sign-ins, failed logins, sign-ups,
  devices added and revoked, refresh-token reuse, password changes,
  grants changed, passkeys added and removed). Users see their own
  through `GET /v1/account/audit`.
- **Disk**: queues are bounded per device and per account, and the relay
  refuses new envelopes below `RELAY_MIN_FREE_DISK_MB`; watch free space
  anyway (the WAL file and backups grow too).
- **Housekeeping**: every 5 minutes the relay deletes expired tokens,
  codes, ceremony state, envelopes older than 24 h and accounts never
  verified within 7 days.
- **Health**: `GET /healthz`.
- **Firewall**: only 80 and 443 open; the relay port is not published.

## 6. Local development

```sh
RELAY_PUBLIC_URL=http://localhost:8080 RELAY_MAILER=log RELAY_LOG_FORMAT=text \
  cargo run -p hame-relay
```

The `log` mailer prints verification and reset links to the console.
